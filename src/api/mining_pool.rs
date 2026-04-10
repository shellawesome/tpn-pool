use crate::cache::TtlCache;
use crate::config::AppConfig;
use crate::db::workers::{get_workers, GetWorkersParams};
use crate::db::DbPool;
use crate::networking::worker::{get_config_from_worker, WorkerResponseBody};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

pub enum LeaseResponseBody {
    Json(Value),
    Text(String),
}

pub struct LeaseResponse {
    pub body: LeaseResponseBody,
    pub lease_ref: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub worker_ip: Option<String>,
    pub country: Option<String>,
    pub connection_type: Option<String>,
}

pub struct LeaseRequestParams<'a> {
    pub pool: &'a DbPool,
    pub config: &'a AppConfig,
    pub cache: &'a Arc<TtlCache>,
    pub lease_seconds: i64,
    pub pinned_worker_ip: Option<&'a str>,
    pub geo: &'a str,
    pub config_type: &'a str,
    pub whitelist: Option<&'a [String]>,
    pub blacklist: Option<&'a [String]>,
    pub priority: bool,
    pub response_format: &'a str,
    pub connection_type: &'a str,
    pub feedback_url: Option<&'a str>,
    pub extend_ref: Option<&'a str>,
    pub extend_expires_at: Option<&'a str>,
}

/// Get a worker config as a miner (fetch from managed workers).
pub async fn get_worker_config_as_miner(p: &LeaseRequestParams<'_>) -> Result<LeaseResponse> {
    // Get candidate workers
    let pinned_worker = p.pinned_worker_ip.is_some();
    let workers = get_workers(
        p.pool,
        &GetWorkersParams {
            mining_pool_uid: Some("internal".to_string()),
            status: Some("up".to_string()),
            worker_ip: p.pinned_worker_ip.map(str::to_string),
            country_code: (!pinned_worker).then(|| p.geo.to_string()),
            connection_type: (!pinned_worker).then(|| p.connection_type.to_string()),
            whitelist: (!pinned_worker)
                .then(|| p.whitelist.map(|v| v.to_vec()))
                .flatten(),
            blacklist: (!pinned_worker)
                .then(|| p.blacklist.map(|v| v.to_vec()))
                .flatten(),
            randomize: !pinned_worker,
            limit: Some(if pinned_worker { 1 } else { 20 }),
        },
    )?;

    if workers.is_empty() {
        return Err(anyhow::anyhow!("No workers available for geo: {}", p.geo));
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    let base_feedback_url = format!("{}/api/request/{}", p.config.base_url(), request_id);
    if let Some(upstream_feedback_url) = p.feedback_url {
        p.cache.set(
            &format!("request_upstream_{}", request_id),
            json!({ "url": upstream_feedback_url }),
            Some(120_000),
        );
    }

    let trace_id = extract_trace_id(p.feedback_url).unwrap_or_else(|| request_id.clone());
    let worker_chunks = workers.chunks(10);

    for worker_chunk in worker_chunks {
        let mut join_set = JoinSet::new();
        for worker in worker_chunk {
            let worker = worker.clone();
            let worker_nonce = uuid::Uuid::new_v4().to_string();
            let query = build_worker_query(
                p.config_type,
                p.lease_seconds,
                p.response_format,
                p.priority,
                &base_feedback_url,
                &worker_nonce,
                &trace_id,
                p.extend_ref,
                p.extend_expires_at,
            );
            let expect_json = p.response_format == "json";
            join_set.spawn(async move {
                let result =
                    get_config_from_worker(&worker.ip, &worker.public_port, &query, expect_json)
                        .await;
                (worker, worker_nonce, result)
            });
        }

        while let Some(joined) = join_set.join_next().await {
            let Ok((worker, worker_nonce, result)) = joined else {
                continue;
            };
            match result {
                Ok((WorkerResponseBody::Json(result), lease_ref, lease_expires))
                    if result.get("error").is_none() =>
                {
                    let mut response = result;
                    if let Some(ref lr) = lease_ref {
                        response["lease_ref"] = Value::String(lr.clone());
                    }
                    if let Some(le) = lease_expires {
                        response["lease_expires_at"] = Value::Number(le.into());
                    }
                    response["country"] = Value::String(worker.country_code.clone());
                    response["connection_type"] = Value::String(worker.connection_type.clone());

                    join_set.abort_all();
                    mark_request_complete(p.cache, &request_id, &worker_nonce);
                    info!("Got config from worker {} for geo {}", worker.ip, p.geo);
                    return Ok(LeaseResponse {
                        body: LeaseResponseBody::Json(response),
                        lease_ref,
                        lease_expires_at: lease_expires,
                        worker_ip: Some(worker.ip.clone()),
                        country: Some(worker.country_code.clone()),
                        connection_type: Some(worker.connection_type.clone()),
                    });
                }
                Ok((WorkerResponseBody::Text(result), lease_ref, lease_expires))
                    if !result.trim().is_empty() =>
                {
                    join_set.abort_all();
                    mark_request_complete(p.cache, &request_id, &worker_nonce);
                    info!("Got config from worker {} for geo {}", worker.ip, p.geo);
                    return Ok(LeaseResponse {
                        body: LeaseResponseBody::Text(result),
                        lease_ref,
                        lease_expires_at: lease_expires,
                        worker_ip: Some(worker.ip.clone()),
                        country: Some(worker.country_code.clone()),
                        connection_type: Some(worker.connection_type.clone()),
                    });
                }
                Ok(_) => {}
                Err(e) => {
                    debug!("Failed to get config from worker {}: {}", worker.ip, e);
                }
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to get config from any worker for geo: {}",
        p.geo
    ))
}

fn build_worker_query(
    config_type: &str,
    lease_seconds: i64,
    response_format: &str,
    priority: bool,
    base_feedback_url: &str,
    worker_nonce: &str,
    trace_id: &str,
    extend_ref: Option<&str>,
    extend_expires_at: Option<&str>,
) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("type", config_type);
    serializer.append_pair("lease_seconds", &lease_seconds.to_string());
    serializer.append_pair("format", response_format);
    serializer.append_pair("priority", if priority { "true" } else { "false" });
    serializer.append_pair(
        "feedback_url",
        &format!(
            "{}?nonce={}&trace={}",
            base_feedback_url, worker_nonce, trace_id
        ),
    );
    if let Some(er) = extend_ref {
        serializer.append_pair("extend_ref", er);
    }
    if let Some(eea) = extend_expires_at {
        serializer.append_pair("extend_expires_at", eea);
    }
    serializer.finish()
}

fn extract_trace_id(upstream_feedback_url: Option<&str>) -> Option<String> {
    let url = upstream_feedback_url?;
    let parsed = url::Url::parse(url).ok()?;
    parsed
        .query_pairs()
        .find(|(k, _)| k == "trace")
        .map(|(_, v)| v.to_string())
}

fn mark_request_complete(cache: &Arc<TtlCache>, request_id: &str, winner_nonce: &str) {
    cache.set(
        &format!("request_{}", request_id),
        json!({ "status": "complete", "winner": winner_nonce }),
        Some(60_000),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::TtlCache;

    #[test]
    fn request_completion_persists_real_winner_nonce() {
        let cache = Arc::new(TtlCache::new());
        mark_request_complete(&cache, "req-1", "nonce-123");

        assert_eq!(
            cache.get("request_req-1"),
            Some(json!({ "status": "complete", "winner": "nonce-123" }))
        );
    }

    #[test]
    fn trace_id_is_extracted_from_upstream_feedback_url() {
        let trace = extract_trace_id(Some(
            "http://example.com/api/request/abc?nonce=winner&trace=trace-42",
        ));
        assert_eq!(trace.as_deref(), Some("trace-42"));
    }
}

/// Format an error together with its full source chain (reqwest's Display hides the cause).
fn format_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(e) = source {
        out.push_str(": ");
        out.push_str(&e.to_string());
        source = e.source();
    }
    out
}

/// Attempt a single POST registration and return Ok(url) on success or Err(reason).
async fn post_to_validator(
    client: &reqwest::Client,
    url: &str,
    payload: &Value,
) -> std::result::Result<(), String> {
    let resp = client
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("request error: {}", format_error_chain(&e)))?;

    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);

    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, body));
    }

    if let Some(err) = body.get("error").and_then(|v| v.as_str()) {
        return Err(format!("validator error: {}", err));
    }

    let success = body
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !success {
        return Err(format!("response did not indicate success: {}", body));
    }

    Ok(())
}

/// Register this mining pool with validators.
pub async fn register_mining_pool_with_validators(
    config: &AppConfig,
    cache: &Arc<TtlCache>,
) -> Result<Vec<String>> {
    let validators = crate::networking::validators::get_validators(cache);
    if validators.is_empty() {
        info!("No known validators yet — waiting for Python shim neuron broadcast");
        return Ok(Vec::new());
    }

    let base_url = config.base_url();
    let payload = json!({
        "protocol": config.server_public_protocol,
        "url": base_url,
        "port": config.server_public_port,
    });

    info!(
        "Registering mining pool with {} validators: {}",
        validators.len(),
        payload
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    let mut successes: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (uid, ip) in &validators {
        let url =
            discover_validator_endpoint(&client, ip, "/validator/broadcast/mining_pool").await;
        info!("Registering mining pool at {} (validator {}@{})", url, uid, ip);
        match post_to_validator(&client, &url, &payload).await {
            Ok(()) => {
                info!("Registered mining pool with validator {}@{} at {}", uid, ip, url);
                successes.push(url);
            }
            Err(reason) => {
                warn!(
                    "Failed to register mining pool with validator {}@{} at {}: {}",
                    uid, ip, url, reason
                );
                failures.push(format!("{}@{}: {}", uid, ip, reason));
            }
        }
    }

    info!(
        "Registered mining pool successfully with {} validators, failed: {}",
        successes.len(),
        failures.len()
    );
    if !failures.is_empty() {
        debug!("Failed registrations: {:?}", failures);
    }

    Ok(successes)
}

/// Broadcast worker data to validators.
pub async fn register_mining_pool_workers_with_validators(
    pool: &DbPool,
    _config: &AppConfig,
    cache: &Arc<TtlCache>,
) -> Result<()> {
    let workers = get_workers(
        pool,
        &GetWorkersParams {
            mining_pool_uid: Some("internal".to_string()),
            ..Default::default()
        },
    )?;

    let validators = crate::networking::validators::get_validators(cache);
    if validators.is_empty() {
        info!("No known validators yet — skipping worker broadcast");
        return Ok(());
    }
    let payload = json!({ "workers": workers });

    info!(
        "Broadcasting {} workers to {} validators",
        workers.len(),
        validators.len()
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    let mut successes: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for (uid, ip) in &validators {
        let url = discover_validator_endpoint(&client, ip, "/validator/broadcast/workers").await;
        info!(
            "Registering at {} with {} workers (validator {}@{})",
            url,
            workers.len(),
            uid,
            ip
        );
        match post_to_validator(&client, &url, &payload).await {
            Ok(()) => {
                info!(
                    "Registered {} workers with validator {}@{} at {}",
                    workers.len(),
                    uid,
                    ip,
                    url
                );
                successes.push(url);
            }
            Err(reason) => {
                warn!(
                    "Failed to broadcast workers to validator {}@{} at {}: {}",
                    uid, ip, url, reason
                );
                failures.push(format!("{}@{}: {}", uid, ip, reason));
            }
        }
    }

    info!(
        "Registered {} workers with validators, successful: {}, failed: {}",
        workers.len(),
        successes.len(),
        failures.len()
    );
    if !failures.is_empty() {
        debug!("Failed registrations: {:?}", failures);
    }

    Ok(())
}

async fn discover_validator_endpoint(client: &reqwest::Client, ip: &str, path: &str) -> String {
    let root_url = format!("http://{}:3000/", ip);
    match client.get(&root_url).send().await {
        Ok(resp) => {
            let body: Value = resp.json().await.unwrap_or(Value::Null);
            let protocol = body
                .get("SERVER_PUBLIC_PROTOCOL")
                .and_then(|v| v.as_str())
                .unwrap_or("http");
            let host = body
                .get("SERVER_PUBLIC_HOST")
                .and_then(|v| v.as_str())
                .unwrap_or(ip);
            let port = body
                .get("SERVER_PUBLIC_PORT")
                .and_then(|v| v.as_u64())
                .unwrap_or(3000);
            format!("{}://{}:{}{}", protocol, host, port, path)
        }
        Err(e) => {
            debug!(
                "Failed to discover public validator endpoint for {}: {}. Falling back to :3000",
                ip, e
            );
            format!("http://{}:3000{}", ip, path)
        }
    }
}
