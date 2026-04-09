use crate::cache::TtlCache;
use crate::config::AppConfig;
use crate::db::workers::{get_workers, GetWorkersParams};
use crate::db::DbPool;
use crate::networking::worker::{get_config_from_worker, WorkerResponseBody};
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

pub enum LeaseResponseBody {
    Json(Value),
    Text(String),
}

pub struct LeaseResponse {
    pub body: LeaseResponseBody,
    pub lease_ref: Option<String>,
    pub lease_expires_at: Option<i64>,
}

/// Get a worker config as a miner (fetch from managed workers).
pub async fn get_worker_config_as_miner(
    pool: &DbPool,
    config: &AppConfig,
    cache: &Arc<TtlCache>,
    lease_seconds: i64,
    geo: &str,
    config_type: &str,
    whitelist: Option<&[String]>,
    blacklist: Option<&[String]>,
    priority: bool,
    response_format: &str,
    connection_type: &str,
    feedback_url: Option<&str>,
) -> Result<LeaseResponse> {
    // Get candidate workers
    let workers = get_workers(
        pool,
        &GetWorkersParams {
            mining_pool_uid: Some("internal".to_string()),
            status: Some("up".to_string()),
            country_code: Some(geo.to_string()),
            connection_type: Some(connection_type.to_string()),
            whitelist: whitelist.map(|v| v.to_vec()),
            blacklist: blacklist.map(|v| v.to_vec()),
            randomize: true,
            limit: Some(20),
            ..Default::default()
        },
    )?;

    if workers.is_empty() {
        return Err(anyhow::anyhow!("No workers available for geo: {}", geo));
    }

    let request_id = uuid::Uuid::new_v4().to_string();
    let base_feedback_url = format!("{}/api/request/{}", config.base_url(), request_id);
    if let Some(upstream_feedback_url) = feedback_url {
        cache.set(
            &format!("request_upstream_{}", request_id),
            json!({ "url": upstream_feedback_url }),
            Some(120_000),
        );
    }

    // Try workers sequentially (first success wins)
    for worker in &workers {
        let params = {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            serializer.append_pair("type", config_type);
            serializer.append_pair("lease_seconds", &lease_seconds.to_string());
            serializer.append_pair("format", response_format);
            serializer.append_pair("priority", if priority { "true" } else { "false" });
            let worker_feedback_url =
                format!("{}?nonce={}&trace={}", base_feedback_url, uuid::Uuid::new_v4(), request_id);
            serializer.append_pair("feedback_url", &worker_feedback_url);
            serializer.finish()
        };

        match get_config_from_worker(
            &worker.ip,
            &worker.public_port,
            &params,
            response_format == "json",
        )
        .await
        {
            Ok((WorkerResponseBody::Json(result), lease_ref, lease_expires)) => {
                if result.get("error").is_none() {
                    let mut response = result;

                    // Add metadata
                    if let Some(ref lr) = lease_ref {
                        response["lease_ref"] = Value::String(lr.clone());
                    }
                    if let Some(le) = lease_expires {
                        response["lease_expires_at"] = Value::Number(le.into());
                    }
                    response["country"] = Value::String(worker.country_code.clone());
                    response["connection_type"] = Value::String(worker.connection_type.clone());

                    info!(
                        "Got config from worker {} for geo {}",
                        worker.ip, geo
                    );
                    cache.set(
                        &format!("request_{}", request_id),
                        json!({ "status": "complete", "winner": serde_json::Value::Null }),
                        Some(60_000),
                    );
                    return Ok(LeaseResponse {
                        body: LeaseResponseBody::Json(response),
                        lease_ref,
                        lease_expires_at: lease_expires,
                    });
                }
            }
            Ok((WorkerResponseBody::Text(result), lease_ref, lease_expires)) => {
                if !result.trim().is_empty() {
                    info!("Got config from worker {} for geo {}", worker.ip, geo);
                    cache.set(
                        &format!("request_{}", request_id),
                        json!({ "status": "complete", "winner": serde_json::Value::Null }),
                        Some(60_000),
                    );
                    return Ok(LeaseResponse {
                        body: LeaseResponseBody::Text(result),
                        lease_ref,
                        lease_expires_at: lease_expires,
                    });
                }
            }
            Err(e) => {
                debug!(
                    "Failed to get config from worker {}: {}",
                    worker.ip, e
                );
            }
        }
    }

    Err(anyhow::anyhow!(
        "Failed to get config from any worker for geo: {}",
        geo
    ))
}

/// Register this mining pool with validators.
pub async fn register_mining_pool_with_validators(
    config: &AppConfig,
    cache: &Arc<TtlCache>,
) -> Result<Vec<String>> {
    let validators = crate::networking::validators::get_validators(cache);
    let mut successes = Vec::new();

    let base_url = config.base_url();
    let payload = json!({
        "protocol": config.server_public_protocol,
        "url": base_url,
        "port": config.server_public_port,
    });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    for (uid, ip) in &validators {
        let url = format!("http://{}:3000/validator/broadcast/mining_pool", ip);
        match client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                let success = body.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                if success {
                    successes.push(ip.clone());
                    info!("Registered with validator {}@{}", uid, ip);
                }
            }
            Err(e) => debug!("Failed to register with validator {}: {}", ip, e),
        }
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
            status: Some("up".to_string()),
            ..Default::default()
        },
    )?;

    let validators = crate::networking::validators::get_validators(cache);
    let payload = json!({ "workers": workers });

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    for (uid, ip) in &validators {
        let url = format!("http://{}:3000/validator/broadcast/workers", ip);
        match client.post(&url).json(&payload).send().await {
            Ok(_) => debug!("Broadcast workers to validator {}@{}", uid, ip),
            Err(e) => debug!("Failed to broadcast workers to {}: {}", ip, e),
        }
    }

    info!(
        "Broadcast {} workers to {} validators",
        workers.len(),
        validators.len()
    );
    Ok(())
}
