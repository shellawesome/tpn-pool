use crate::db::workers::Worker;
use crate::networking::routing::WORKER_FETCH_TIMEOUT_MS;
use anyhow::Result;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, info};

/// Fetch WireGuard and SOCKS5 configs from workers or through the mining pool.
/// If mining_pool_ip is provided, fetch through the pool; otherwise directly from workers.
pub async fn add_configs_to_workers(
    workers: &[Worker],
    mining_pool_uid: Option<&str>,
    mining_pool_ip: Option<&str>,
    lease_seconds: i64,
) -> Vec<Worker> {
    let mut workers_with_configs = Vec::new();

    for worker in workers {
        let mut w = worker.clone();

        // If fetching through mining pool
        if let (Some(_pool_uid), Some(pool_ip)) = (mining_pool_uid, mining_pool_ip) {
            match fetch_config_through_pool(pool_ip, &worker.ip, lease_seconds).await {
                Ok((wg_config, socks5_config)) => {
                    w.wireguard_config = Some(wg_config.clone());
                    w.socks5_config = socks5_config;
                    // Parse text_config from wireguard_config
                    w.text_config = Some(wg_config);
                    workers_with_configs.push(w);
                }
                Err(e) => {
                    debug!(
                        "Failed to fetch config for worker {} through pool: {}",
                        worker.ip, e
                    );
                }
            }
        } else {
            // Fetch directly from worker
            match fetch_config_from_worker(&worker.ip, &worker.public_port, lease_seconds).await {
                Ok((wg_config, socks5_config)) => {
                    w.wireguard_config = Some(wg_config.clone());
                    w.socks5_config = socks5_config;
                    w.text_config = Some(wg_config);
                    workers_with_configs.push(w);
                }
                Err(e) => {
                    debug!(
                        "Failed to fetch config directly from worker {}: {}",
                        worker.ip, e
                    );
                }
            }
        }
    }

    info!(
        "Fetched configs for {}/{} workers",
        workers_with_configs.len(),
        workers.len()
    );
    workers_with_configs
}

async fn fetch_config_through_pool(
    pool_ip: &str,
    _worker_ip: &str,
    lease_seconds: i64,
) -> Result<(String, Option<String>)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(WORKER_FETCH_TIMEOUT_MS * 2))
        .build()?;

    // Fetch WireGuard config
    let wg_url = format!(
        "http://{}:3000/api/lease/new?type=wireguard&lease_seconds={}&format=text",
        pool_ip, lease_seconds
    );
    let wg_response = client.get(&wg_url).send().await?;
    let wg_status = wg_response.status();
    if !wg_status.is_success() {
        let body = wg_response.text().await.unwrap_or_default();
        anyhow::bail!(
            "pool returned HTTP {} for WireGuard lease request: {}",
            wg_status,
            body
        );
    }
    let wg_config = wg_response.text().await?;
    if wg_config.trim().is_empty() {
        anyhow::bail!("pool returned empty WireGuard config");
    }

    // Fetch SOCKS5 config
    let socks5_url = format!(
        "http://{}:3000/api/lease/new?type=socks5&lease_seconds={}&format=json",
        pool_ip, lease_seconds
    );
    let socks5_config = match client.get(&socks5_url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Ok((wg_config, None));
            }
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            if let (Some(user), Some(pass), Some(ip), Some(port)) = (
                body.get("username").and_then(|v| v.as_str()),
                body.get("password").and_then(|v| v.as_str()),
                body.get("ip_address").and_then(|v| v.as_str()),
                body.get("port").and_then(|v| v.as_i64()),
            ) {
                Some(format!("socks5://{}:{}@{}:{}", user, pass, ip, port))
            } else {
                None
            }
        }
        Err(_) => None,
    };

    Ok((wg_config, socks5_config))
}

async fn fetch_config_from_worker(
    worker_ip: &str,
    public_port: &str,
    lease_seconds: i64,
) -> Result<(String, Option<String>)> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(WORKER_FETCH_TIMEOUT_MS))
        .build()?;

    // Fetch WireGuard config
    let wg_url = format!(
        "http://{}:{}/api/lease/new?type=wireguard&lease_seconds={}&format=text",
        worker_ip, public_port, lease_seconds
    );
    let wg_response = client.get(&wg_url).send().await?;
    let wg_status = wg_response.status();
    if !wg_status.is_success() {
        let body = wg_response.text().await.unwrap_or_default();
        anyhow::bail!(
            "worker returned HTTP {} for WireGuard lease request: {}",
            wg_status,
            body
        );
    }
    let wg_config = wg_response.text().await?;
    if wg_config.trim().is_empty() {
        anyhow::bail!("worker returned empty WireGuard config");
    }

    // Fetch SOCKS5 config
    let socks5_url = format!(
        "http://{}:{}/api/lease/new?type=socks5&lease_seconds={}&format=json",
        worker_ip, public_port, lease_seconds
    );
    let socks5_config = match client.get(&socks5_url).send().await {
        Ok(resp) => {
            if !resp.status().is_success() {
                return Ok((wg_config, None));
            }
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            if let (Some(user), Some(pass), Some(ip), Some(port)) = (
                body.get("username").and_then(|v| v.as_str()),
                body.get("password").and_then(|v| v.as_str()),
                body.get("ip_address").and_then(|v| v.as_str()),
                body.get("port").and_then(|v| v.as_i64()),
            ) {
                Some(format!("socks5://{}:{}@{}:{}", user, pass, ip, port))
            } else {
                None
            }
        }
        Err(_) => None,
    };

    Ok((wg_config, socks5_config))
}
