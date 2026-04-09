use crate::networking::routing::WORKER_FETCH_TIMEOUT_MS;
use anyhow::Result;
use std::time::Duration;
use tracing::debug;

pub enum WorkerResponseBody {
    Json(serde_json::Value),
    Text(String),
}

/// Fetch a config directly from a worker node.
pub async fn get_config_from_worker(
    worker_ip: &str,
    public_port: &str,
    params: &str,
    expect_json: bool,
) -> Result<(WorkerResponseBody, Option<String>, Option<i64>)> {
    let url = format!("http://{}:{}/api/lease/new?{}", worker_ip, public_port, params);
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
