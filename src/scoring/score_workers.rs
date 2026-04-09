use crate::config::AppConfig;
use crate::db::workers::Worker;
use crate::db::DbPool;
use crate::geo::GeoService;
use crate::locks::LockRegistry;
use crate::networking::wireguard::test_wireguard_connection;
use crate::networking::socks5::test_socks5_connection;
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
        validate_and_annotate_workers(&workers_with_configs, config, None, None).await;

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

    // Validate each worker
    let mut handles = Vec::new();
    for worker in workers_with_configs.iter().cloned() {
        let is_partnered = is_partnered;
        handles.push(tokio::spawn(async move {
            validate_single_worker(worker, is_partnered).await
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

    // Also include workers that had invalid configs
    let invalid_workers: Vec<Worker> = workers_with_configs
        .iter()
        .filter(|w| {
            !is_valid_worker(w)
                || w.wireguard_config.is_none()
                || w.wireguard_config.as_deref().map(|c| c.is_empty()).unwrap_or(true)
        })
        .cloned()
        .map(|mut w| {
            w.success = Some(false);
            w.error = Some("Invalid worker or missing config".to_string());
            w.status = "down".to_string();
            w
        })
        .collect();
    failures.extend(invalid_workers);

    (successes, failures)
}

async fn validate_single_worker(mut worker: Worker, is_partnered: bool) -> Worker {
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
    }

    // WireGuard test
    if let Some(ref wg_config) = worker.wireguard_config {
        let wg_result = test_wireguard_connection(wg_config, &worker.ip).await;
        if !wg_result.valid {
            let status = status_from_failure_code(
                wg_result.failure_code.as_deref().unwrap_or("unknown"),
            );
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
