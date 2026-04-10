use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tracing::info;

use crate::db::workers::{get_workers, GetWorkersParams};
use crate::networking::validators::get_validators;
use crate::wallet_files;
use crate::AppState;

use super::auth::verify_request;

/// GET /api/dashboard — Auth-gated aggregated dashboard JSON data.
pub async fn dashboard_data_handler(State(state): State<AppState>) -> impl IntoResponse {
    render_dashboard_data(&state).await.into_response()
}

pub async fn console_dashboard_data_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "unauthorized"})),
        )
            .into_response();
    }

    render_dashboard_data(&state).await.into_response()
}

async fn render_dashboard_data(state: &AppState) -> Json<Value> {
    info!("GET /api/dashboard requested");
    let config = &state.config;
    let uptime_secs = chrono::Utc::now()
        .signed_duration_since(state.start_time)
        .num_seconds();

    let wallet_paths = wallet_files::wallet_paths().ok();
    let hotkey_ss58 = wallet_files::derive_hotkey_ss58(config).ok();
    let coldkey_ss58 = wallet_files::derive_coldkey_ss58(config).ok();
    let chain_registration = get_cached_chain_registration(&state).await;
    let performance_summary = build_performance_summary(&state);

    // Worker count and distribution
    let (
        total_workers,
        workers_by_country,
        workers_by_status,
        workers_by_connection_type,
        registered_pool_count,
        latest_worker_update_ms,
        recent_workers,
    ) = match get_workers(
        &state.db,
        &GetWorkersParams {
            limit: Some(100000),
            ..Default::default()
        },
    ) {
        Ok(workers) => {
            let total = workers.len();
            let mut by_country: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
            let mut by_connection_type: BTreeMap<String, usize> = BTreeMap::new();
            let mut registered_pools = BTreeSet::new();
            let mut latest_updated_at = None;
            let mut recent = workers.clone();
            recent.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            for w in &workers {
                *by_country.entry(w.country_code.clone()).or_default() += 1;
                *by_status.entry(w.status.clone()).or_default() += 1;
                *by_connection_type
                    .entry(w.connection_type.clone())
                    .or_default() += 1;
                if !w.mining_pool_uid.is_empty() {
                    registered_pools.insert(w.mining_pool_uid.clone());
                }
                latest_updated_at = Some(
                    latest_updated_at
                        .map_or(w.updated_at, |current: i64| current.max(w.updated_at)),
                );
            }
            (
                total,
                by_country,
                by_status,
                by_connection_type,
                registered_pools.len(),
                latest_updated_at,
                recent
                    .into_iter()
                    .take(20)
                    .map(|worker| {
                        json!({
                            "ip": worker.ip,
                            "public_url": worker.public_url,
                            "country_code": worker.country_code,
                            "status": worker.status,
                            "connection_type": worker.connection_type,
                            "mining_pool_uid": worker.mining_pool_uid,
                            "updated_at": timestamp_ms_to_rfc3339(worker.updated_at),
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        }
        Err(_) => (
            0,
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            0,
            None,
            Vec::new(),
        ),
    };

    let up_workers = match get_workers(
        &state.db,
        &GetWorkersParams {
            status: Some("up".to_string()),
            limit: Some(100000),
            ..Default::default()
        },
    ) {
        Ok(w) => w.len(),
        Err(_) => 0,
    };

    let cache_keys = state.cache.len();
    let mut dashboard_scores = match state.tpn_cache.get("dashboard_scores").await {
        Some(serde_json::Value::Array(entries)) => entries,
        _ => Vec::new(),
    };
    dashboard_scores.sort_by(|a, b| {
        let a_score = a
            .get("score")
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        let b_score = b
            .get("score")
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let validators = get_validators(&state.cache);
    let validator_count = validators.len();

    let request_entries = state.cache.entries_with_prefix("request_");
    let request_values: Vec<_> = request_entries
        .iter()
        .filter(|(key, _)| {
            !key.starts_with("request_upstream_") && !key.starts_with("request_upstream_checked_")
        })
        .map(|(_, value)| value)
        .collect();
    let upstream_request_values = state.cache.entries_with_prefix("request_upstream_");
    let mut request_status_counts: HashMap<String, usize> = HashMap::new();
    let mut complete_requests = 0usize;
    for value in &request_values {
        let status = value
            .get("status")
            .and_then(|status| status.as_str())
            .unwrap_or("unknown")
            .to_string();
        *request_status_counts.entry(status.clone()).or_default() += 1;
        if status == "complete" {
            complete_requests += 1;
        }
    }

    Json(json!({
        "node": {
            "version": env!("CARGO_PKG_VERSION"),
            "mode": "miner",
            "pid": std::process::id(),
            "uptime_seconds": uptime_secs,
            "start_time": state.start_time.to_rfc3339(),
            "git_branch": state.branch,
            "git_hash": state.hash,
        },
        "mining_pool": {
            "url": config.mining_pool_url,
            "name": config.mining_pool_name,
            "website_url": config.mining_pool_website_url,
            "rewards_url": config.mining_pool_rewards,
        },
        "network": {
            "public_host": config.server_public_host,
            "public_port": config.server_public_port,
            "protocol": config.server_public_protocol,
            "base_url": config.base_url(),
            "external_ip": config.bt_external_ip,
            "internal_subnet": config.tpn_internal_subnet,
            "external_subnet": config.tpn_external_subnet,
        },
        "chain": {
            "network": config.bt_subtensor_network,
            "endpoint": config.bt_subtensor_chain_endpoint,
            "netuid": config.bt_netuid,
            "axon_port": config.bt_axon_port,
            "registered": chain_registration.registered,
            "uid": chain_registration.uid,
            "status_checked_at": chain_registration.checked_at,
            "status_error": chain_registration.error,
        },
        "wallet": {
            "wallet_name": wallet_files::DEFAULT_WALLET_NAME,
            "hotkey_name": wallet_files::DEFAULT_HOTKEY_NAME,
            "hotkey_ss58": hotkey_ss58,
            "coldkey_ss58": coldkey_ss58,
            "wallet_dir": wallet_paths.as_ref().map(|paths| paths.wallet_dir.display().to_string()),
            "hotkey_path": wallet_paths.as_ref().map(|paths| paths.hotkey_path.display().to_string()),
            "coldkey_path": wallet_paths.as_ref().map(|paths| paths.coldkey_path.display().to_string()),
        },
        "payment": {
            "evm_address": config.payment_address_evm,
            "bittensor_address": config.payment_address_bittensor,
        },
        "validators": {
            "known": validator_count,
        },
        "workers": {
            "total": total_workers,
            "up": up_workers,
            "by_country": workers_by_country,
            "by_status": workers_by_status,
            "by_connection_type": workers_by_connection_type,
            "registered_pools": registered_pool_count,
            "last_updated_at": latest_worker_update_ms.map(timestamp_ms_to_rfc3339),
            "recent": recent_workers,
        },
        "requests": {
            "tracked": request_values.len(),
            "complete": complete_requests,
            "upstream_watch": upstream_request_values.len(),
            "by_status": request_status_counts,
        },
        "performance": performance_summary,
        "scores": {
            "pools": dashboard_scores,
        },
        "cache": {
            "keys": cache_keys,
        },
        "database": {
            "path": config.db_path,
        },
    }))
}

fn timestamp_ms_to_rfc3339(timestamp_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|ts| ts.to_rfc3339())
        .unwrap_or_else(|| timestamp_ms.to_string())
}

struct ChainRegistrationSummary {
    registered: bool,
    uid: Option<u16>,
    checked_at: Option<String>,
    error: Option<String>,
}

async fn get_cached_chain_registration(state: &AppState) -> ChainRegistrationSummary {
    if let Some(cached) = state.tpn_cache.get("dashboard_chain_registration").await {
        if let Some(checked_at_ms) = cached.get("checked_at_ms").and_then(|value| value.as_i64()) {
            let now_ms = chrono::Utc::now().timestamp_millis();
            if now_ms - checked_at_ms < 60_000 {
                return ChainRegistrationSummary {
                    registered: cached
                        .get("registered")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                    uid: cached
                        .get("uid")
                        .and_then(|value| value.as_u64())
                        .map(|v| v as u16),
                    checked_at: cached
                        .get("checked_at")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned),
                    error: cached
                        .get("error")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned),
                };
            }
        }
    }

    let now = chrono::Utc::now();
    let fresh = match crate::chain::fetch_existing_uid(&state.config).await {
        Ok(uid) => json!({
            "registered": uid.is_some(),
            "uid": uid,
            "checked_at": now.to_rfc3339(),
            "checked_at_ms": now.timestamp_millis(),
            "error": Value::Null,
        }),
        Err(error) => json!({
            "registered": false,
            "uid": Value::Null,
            "checked_at": now.to_rfc3339(),
            "checked_at_ms": now.timestamp_millis(),
            "error": error.to_string(),
        }),
    };

    state
        .tpn_cache
        .set("dashboard_chain_registration", fresh.clone())
        .await;

    ChainRegistrationSummary {
        registered: fresh
            .get("registered")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        uid: fresh
            .get("uid")
            .and_then(|value| value.as_u64())
            .map(|v| v as u16),
        checked_at: fresh
            .get("checked_at")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        error: fresh
            .get("error")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
    }
}

fn build_performance_summary(state: &AppState) -> serde_json::Value {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = now_ms - 24 * 60 * 60 * 1000;
    let records = match crate::db::workers::get_worker_performance(&state.db, Some(from_ms), None) {
        Ok(records) => records,
        Err(error) => {
            return json!({
                "window_hours": 24,
                "samples": 0,
                "unique_workers": 0,
                "up_samples": 0,
                "up_ratio": Value::Null,
                "last_sample_at": Value::Null,
                "hourly": [],
                "error": error.to_string(),
            });
        }
    };

    let samples = records.len();
    let unique_workers = records
        .iter()
        .map(|(ip, _, _, _)| ip.clone())
        .collect::<BTreeSet<_>>()
        .len();
    let up_samples = records
        .iter()
        .filter(|(_, status, _, _)| status == "up")
        .count();
    let last_sample_at = records
        .last()
        .map(|(_, _, _, updated_at)| timestamp_ms_to_rfc3339(*updated_at));
    let mut hourly: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (_, status, _, updated_at) in &records {
        if let Some(ts) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*updated_at) {
            let bucket = ts.format("%m-%d %H:00").to_string();
            let entry = hourly.entry(bucket).or_insert((0, 0));
            entry.0 += 1;
            if status == "up" {
                entry.1 += 1;
            }
        }
    }
    let hourly = hourly
        .into_iter()
        .map(|(bucket, (samples, up_samples))| {
            let up_ratio = if samples > 0 {
                Some(((up_samples as f64 / samples as f64) * 1000.0).round() / 1000.0)
            } else {
                None
            };
            json!({
                "bucket": bucket,
                "samples": samples,
                "up_samples": up_samples,
                "up_ratio": up_ratio,
            })
        })
        .collect::<Vec<_>>();
    let up_ratio = if samples > 0 {
        Some(((up_samples as f64 / samples as f64) * 1000.0).round() / 1000.0)
    } else {
        None
    };

    json!({
        "window_hours": 24,
        "samples": samples,
        "unique_workers": unique_workers,
        "up_samples": up_samples,
        "up_ratio": up_ratio,
        "last_sample_at": last_sample_at,
        "hourly": hourly,
        "error": Value::Null,
    })
}
