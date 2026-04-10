use crate::config::AppConfig;
use crate::db::workers::Worker;
use crate::db::DbPool;
use crate::geo::GeoService;
use crate::locks::LockRegistry;
use crate::networking::socks5::test_socks5_connection;
use crate::networking::wireguard::test_wireguard_connection;
use crate::networking::worker::get_worker_claimed_pool_url;
use crate::partnered_pools::is_partnered_pool;
use crate::scoring::score_node::score_node_version;
use crate::validations::is_valid_worker;
use anyhow::Result;
use std::sync::Arc;
use tracing::{info, warn};

fn status_from_failure_code(code: &str) -> &str {
    if code == "egress_ip_mismatch" {
        "cheat"
    } else {
        "down"
    }
}

/// Score all known workers (miner mode).
pub async fn score_all_known_workers(
    pool: &DbPool,
    config: &AppConfig,
    locks: &Arc<LockRegistry>,
    _geo: &Arc<GeoService>,
) -> Result<()> {
    let guard = locks.try_acquire("score_all_known_workers");
    if guard.is_none() {
        warn!("score_all_known_workers is already running");
        return Ok(());
    }

    info!("Starting score_all_known_workers");

    // Get all known workers
    let workers = crate::db::workers::get_workers(
        pool,
        &crate::db::workers::GetWorkersParams {
            mining_pool_uid: Some("internal".to_string()),
            ..Default::default()
        },
    )?;

    if workers.is_empty() {
        info!("No known workers to score");
        return Ok(());
    }

    // Fetch configs from workers
    let workers_with_configs =
        crate::scoring::query_workers::add_configs_to_workers(&workers, None, None, 120).await;

    // Validate workers
    let (successes, failures) =
        validate_and_annotate_workers(&workers_with_configs, config, pool, None, None).await;

    // Build annotated list
    let mut annotated: Vec<Worker> = successes
        .into_iter()
        .map(|mut w| {
            w.status = "up".to_string();
            w
        })
        .collect();
    annotated.extend(failures.into_iter().map(|mut w| {
        if w.status == "unknown" {
            w.status = "down".to_string();
        }
        w
    }));

    // Save to database
    crate::db::workers::write_workers(pool, &annotated, "internal", "")?;
    crate::db::workers::write_worker_performance(pool, &annotated)?;

    let up_count = annotated.iter().filter(|w| w.status == "up").count();
    let down_count = annotated.len() - up_count;
    info!(
        "Scored all known workers: {} up, {} down/cheat",
        up_count, down_count
    );

    Ok(())
}

/// Validate and annotate workers with test results.
pub async fn validate_and_annotate_workers(
    workers_with_configs: &[Worker],
    config: &AppConfig,
    db: &DbPool,
    mining_pool_uid: Option<&str>,
    mining_pool_ip: Option<&str>,
) -> (Vec<Worker>, Vec<Worker>) {
    let is_partnered = mining_pool_uid
        .zip(mining_pool_ip)
        .map(|(uid, ip)| is_partnered_pool(config, uid, ip))
        .unwrap_or(false);

    if is_partnered {
        info!(
            "Pool {} is partnered, skipping version and membership checks",
            mining_pool_uid.unwrap_or("?")
        );
    }

    let mut successes = Vec::new();
    let mut failures = Vec::new();

    // Validate each worker with a syntactically valid WireGuard config.
    let mut handles = Vec::new();
    let base_url = config.base_url();
    let mut invalid_workers = Vec::new();
    for worker in workers_with_configs.iter().cloned() {
        let valid_worker = is_valid_worker(&worker);
        let has_wg_config = worker
            .wireguard_config
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
        let parsed_wg = worker
            .wireguard_config
            .as_deref()
            .map(crate::networking::wireguard::parse_wireguard_config);
        let wg_valid = parsed_wg.as_ref().map(|p| p.config_valid).unwrap_or(false);

        if !valid_worker || !has_wg_config || !wg_valid {
            let mut invalid = worker.clone();
            invalid.success = Some(false);
            invalid.error = Some("Invalid worker or missing/invalid WireGuard config".to_string());
            invalid.status = "down".to_string();
            invalid_workers.push(invalid);
            continue;
        }

        let ci_mode = config.ci_mode;
        let db_clone = db.clone();
        let base_url_clone = base_url.clone();
        handles.push(tokio::spawn(async move {
            validate_single_worker(worker, is_partnered, ci_mode, &db_clone, &base_url_clone).await
        }));
    }

    for handle in handles {
        match handle.await {
            Ok(worker) => {
                if worker.success == Some(true) {
                    successes.push(worker);
                } else {
                    failures.push(worker);
                }
            }
            Err(e) => warn!("Worker validation task panicked: {}", e),
        }
    }

    failures.extend(invalid_workers);

    (successes, failures)
}

async fn validate_single_worker(
    mut worker: Worker,
    is_partnered: bool,
    ci_mode: bool,
    db: &DbPool,
    pool_base_url: &str,
) -> Worker {
    let start = std::time::Instant::now();

    // Version check (skip for partnered)
    if !is_partnered {
        let port: u16 = worker.public_port.parse().unwrap_or(3000);
        match score_node_version(&worker.ip, port, worker.public_url.as_deref()).await {
            Ok((valid, version)) => {
                if !valid {
                    worker.success = Some(false);
                    worker.error = Some(format!("Outdated version: {}", version));
                    worker.status = "down".to_string();
                    worker.test_duration_s = Some(start.elapsed().as_secs_f64());
                    return worker;
                }
            }
            Err(e) => {
                worker.success = Some(false);
                worker.error = Some(format!("Version check failed: {}", e));
                worker.status = "down".to_string();
                worker.test_duration_s = Some(start.elapsed().as_secs_f64());
                return worker;
            }
        }

        match worker_claims_expected_pool(&worker, ci_mode).await {
            Ok(true) => {}
            Ok(false) => {
                worker.success = Some(false);
                worker.error = Some(format!(
                    "Worker does not claim expected mining pool {}",
                    worker.mining_pool_url
                ));
                worker.status = "down".to_string();
                worker.test_duration_s = Some(start.elapsed().as_secs_f64());
                return worker;
            }
            Err(e) => {
                worker.success = Some(false);
                worker.error = Some(format!("Membership check failed: {}", e));
                worker.status = "down".to_string();
                worker.test_duration_s = Some(start.elapsed().as_secs_f64());
                return worker;
            }
        }
    }

    // WireGuard test
    if let Some(ref wg_config) = worker.wireguard_config {
        let wg_result = test_wireguard_connection(wg_config, &worker.ip, pool_base_url, db).await;
        if !wg_result.valid {
            let status =
                status_from_failure_code(wg_result.failure_code.as_deref().unwrap_or("unknown"));
            worker.success = Some(false);
            worker.status = status.to_string();
            worker.failure_code = wg_result.failure_code;
            worker.observed_egress_ip = wg_result.observed_egress_ip;
            worker.error = Some(wg_result.message);
            worker.test_duration_s = Some(start.elapsed().as_secs_f64());
            return worker;
        }
    }

    // SOCKS5 test
    if let Some(ref sock) = worker.socks5_config {
        let socks5_result = test_socks5_connection(sock, Some(&worker.ip)).await;
        if !socks5_result.valid {
            let status = status_from_failure_code(
                socks5_result.failure_code.as_deref().unwrap_or("unknown"),
            );
            worker.success = Some(false);
            worker.status = status.to_string();
            worker.failure_code = socks5_result.failure_code;
            worker.error = Some(socks5_result.message);
            worker.test_duration_s = Some(start.elapsed().as_secs_f64());
            return worker;
        }
    }

    // All tests passed
    worker.success = Some(true);
    worker.status = "up".to_string();
    worker.test_duration_s = Some(start.elapsed().as_secs_f64());
    worker
}

async fn worker_claims_expected_pool(worker: &Worker, ci_mode: bool) -> Result<bool> {
    let claimed_pool_url = get_worker_claimed_pool_url(&worker.ip, &worker.public_port).await?;
    Ok(worker_claim_matches_expected(
        &claimed_pool_url,
        &worker.mining_pool_url,
        ci_mode,
    ))
}

fn worker_claim_matches_expected(
    claimed_pool_url: &str,
    expected_pool_url: &str,
    ci_mode: bool,
) -> bool {
    ci_mode || claimed_pool_url == expected_pool_url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_check_accepts_exact_match() {
        assert!(worker_claim_matches_expected(
            "http://pool",
            "http://pool",
            false
        ));
    }

    #[test]
    fn membership_check_rejects_mismatch_outside_ci() {
        assert!(!worker_claim_matches_expected(
            "http://other-pool",
            "http://pool",
            false
        ));
    }

    #[test]
    fn membership_check_allows_mismatch_in_ci() {
        assert!(worker_claim_matches_expected(
            "http://other-pool",
            "http://pool",
            true
        ));
    }
}
