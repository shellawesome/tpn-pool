use crate::networking::routing::WORKER_FETCH_TIMEOUT_MS;
use anyhow::Result;
use std::time::Duration;
use tracing::debug;

pub enum WorkerResponseBody {
    Json(serde_json::Value),
    Text(String),
}

/// Fetch a config directly from a worker node.
/// Supports extend_ref and extend_expires_at for lease extension forwarding.
pub async fn get_config_from_worker(
    worker_ip: &str,
    public_port: &str,
    params: &str,
    expect_json: bool,
) -> Result<(WorkerResponseBody, Option<String>, Option<i64>)> {
    let url = format!(
        "http://{}:{}/api/lease/new?{}",
        worker_ip, public_port, params
    );
    debug!("Fetching config from worker: {}", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(WORKER_FETCH_TIMEOUT_MS))
        .build()?;

    let response = client.get(&url).send().await?;

    // Read lease headers
    let lease_ref = response
        .headers()
        .get("X-Lease-Ref")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let lease_expires = response
        .headers()
        .get("X-Lease-Expires")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());

    let body = if expect_json {
        WorkerResponseBody::Json(response.json().await?)
    } else {
        WorkerResponseBody::Text(response.text().await?)
    };

    Ok((body, lease_ref, lease_expires))
}

/// Read the worker's claimed mining pool URL from its root metadata endpoint.
pub async fn get_worker_claimed_pool_url(worker_ip: &str, public_port: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(WORKER_FETCH_TIMEOUT_MS))
        .build()?;

    let url = format!("http://{}:{}", worker_ip, public_port);
    let body: serde_json::Value = client.get(&url).send().await?.json().await?;
    body.get("MINING_POOL_URL")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("worker does not expose MINING_POOL_URL"))
}
