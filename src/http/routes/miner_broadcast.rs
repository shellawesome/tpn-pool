use axum::{
    extract::{ConnectInfo, State},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tracing::{info, warn};

use crate::db::workers::{find_clashing_workers, write_workers, Worker};
use crate::networking::network::ip_from_request;
use crate::networking::socks5::test_socks5_connection;
use crate::networking::validators::is_validator_request;
use crate::networking::wireguard::test_wireguard_connection;
use crate::networking::worker::get_worker_claimed_pool_url;
use crate::scoring::score_node::score_node_version;
use crate::validations::is_valid_worker;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/miner/broadcast/worker", post(register_worker))
        .route("/miner/broadcast/worker/feedback", post(worker_feedback))
}

async fn register_worker(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let unspoofable_ip = ip_from_request(&addr);
    info!("Worker registration from {}", unspoofable_ip);

    // Parse worker data from payload
    let mut worker = Worker {
        ip: unspoofable_ip.clone(),
        public_url: payload
            .get("public_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        payment_address_evm: payload
            .get("payment_address_evm")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        payment_address_bittensor: payload
            .get("payment_address_bittensor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        public_port: payload
            .get("public_port")
            .and_then(|v| v.as_str().or_else(|| v.as_i64().map(|_| "")))
            .unwrap_or("3000")
            .to_string(),
        mining_pool_url: payload
            .get("mining_pool_url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        mining_pool_uid: "internal".to_string(),
        status: "unknown".to_string(),
        country_code: "XX".to_string(),
        connection_type: "unknown".to_string(),
        wireguard_config: payload
            .get("wireguard_config")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        socks5_config: payload
            .get("socks5_config")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        ..Default::default()
    };

    // Fix public_port if it came as a number
    if let Some(port_num) = payload.get("public_port").and_then(|v| v.as_i64()) {
        worker.public_port = port_num.to_string();
    }

    if !is_valid_worker(&worker) {
        return Json(json!({"success": false, "error": "Invalid worker data"}));
    }

    // Geo lookup
    let geodata = state.geo.lookup(&state.db, &unspoofable_ip).await;
    worker.country_code = geodata.country_code;
    worker.connection_type = geodata.connection_type;

    // Validate claimed pool membership before accepting the worker.
    if !worker.mining_pool_url.is_empty() {
        match get_worker_claimed_pool_url(&worker.ip, &worker.public_port).await {
            Ok(claimed_pool_url) => {
                let pool_matches =
                    state.config.ci_mode || claimed_pool_url == worker.mining_pool_url;
                if !pool_matches {
                    return Json(json!({
                        "success": false,
                        "error": format!(
                            "Worker claims mining pool {} but expected {}",
                            claimed_pool_url,
                            worker.mining_pool_url
                        )
                    }));
                }
            }
            Err(e) => {
                return Json(json!({
                    "success": false,
                    "error": format!("Failed to verify worker mining pool membership: {}", e),
                }));
            }
        }
    }

    // Validate the worker version before marking it as available.
    let port: u16 = worker.public_port.parse().unwrap_or(3000);
    match score_node_version(&worker.ip, port, worker.public_url.as_deref()).await {
        Ok((true, _)) => {}
        Ok((false, version)) => {
            return Json(json!({
                "success": false,
                "error": format!("Worker is running an outdated or invalid version: {}", version),
            }));
        }
        Err(e) => {
            return Json(json!({
                "success": false,
                "error": format!("Failed to validate worker version: {}", e),
            }));
        }
    }

    // Validate the advertised connection configs up front so only working workers become 'up'.
    let has_wireguard = worker
        .wireguard_config
        .as_deref()
        .map(|cfg| !cfg.trim().is_empty())
        .unwrap_or(false);
    let has_socks5 = worker
        .socks5_config
        .as_deref()
        .map(|cfg| !cfg.trim().is_empty())
        .unwrap_or(false);
    if !has_wireguard && !has_socks5 {
        return Json(json!({
            "success": false,
            "error": "Worker registration must include wireguard_config or socks5_config",
        }));
    }

    if let Some(ref wg_config) = worker.wireguard_config {
        let wg_result =
            test_wireguard_connection(wg_config, &worker.ip, &state.config.base_url(), &state.db)
                .await;
        if !wg_result.valid {
            return Json(json!({
                "success": false,
                "error": format!("WireGuard validation failed: {}", wg_result.message),
            }));
        }
    }

    if let Some(ref socks5_config) = worker.socks5_config {
        let socks5_result = test_socks5_connection(socks5_config, Some(&worker.ip)).await;
        if !socks5_result.valid {
            return Json(json!({
                "success": false,
                "error": format!("SOCKS5 validation failed: {}", socks5_result.message),
            }));
        }
    }

    // Check for IP clashes
    let clashes =
        find_clashing_workers(&state.db, &[worker.clone()], "internal").unwrap_or_default();
    if !clashes.is_empty() {
        warn!(
            "IP clash detected for {}: {} existing workers",
            unspoofable_ip,
            clashes.len()
        );
        return Json(json!({
            "success": false,
            "error": "Worker IP conflicts with an active worker from another pool",
        }));
    }

    // Set status to up
    worker.status = "up".to_string();

    // Save worker
    match write_workers(&state.db, &[worker.clone()], "internal", "") {
        Ok(()) => {
            info!(
                "Registered worker {} from {}",
                unspoofable_ip, worker.country_code
            );
            Json(json!({
                "registered": true,
                "success": true,
                "worker": {
                    "ip": worker.ip,
                    "country_code": worker.country_code,
                    "connection_type": worker.connection_type,
                }
            }))
        }
        Err(e) => {
            warn!("Failed to register worker {}: {}", unspoofable_ip, e);
            Json(json!({"success": false, "error": e.to_string()}))
        }
    }
}

async fn worker_feedback(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    let unspoofable_ip = ip_from_request(&addr);
    let validator = is_validator_request(&unspoofable_ip, &state.cache);
    let Some((validator_uid, validator_ip)) = validator else {
        return Json(json!({"success": false, "error": "Forbidden, endpoint only for validators"}));
    };

    info!(
        "Received worker feedback from validator {} ({})",
        validator_uid, validator_ip
    );

    // Process composite scores if present
    if let Some(scores) = payload.get("composite_scores") {
        let score = scores.get("score").cloned().unwrap_or(Value::Null);
        let stability = scores.get("stability_score").cloned().unwrap_or(Value::Null);
        let size = scores.get("size_score").cloned().unwrap_or(Value::Null);
        let performance = scores
            .get("performance_score")
            .cloned()
            .unwrap_or(Value::Null);
        let geo = scores.get("geo_score").cloned().unwrap_or(Value::Null);
        info!(
            "Validator {} composite scores: score={} stability={} size={} performance={} geo={}",
            validator_uid, score, stability, size, performance, geo,
        );
        persist_validator_score_feedback(&state, &validator_uid, &validator_ip, scores.clone())
            .await;
    }

    // Process worker status updates
    if let Some(workers) = payload
        .get("workers_with_status")
        .and_then(|v| v.as_array())
    {
        // Log per-worker scoring results from validator before deserializing
        let mut up = 0usize;
        let mut down = 0usize;
        let mut cheat = 0usize;
        for w in workers {
            let ip = w.get("ip").and_then(|v| v.as_str()).unwrap_or("?");
            let status = w.get("status").and_then(|v| v.as_str()).unwrap_or("?");
            let failure_code = w
                .get("failure_code")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error = w.get("error").and_then(|v| v.as_str()).unwrap_or("");
            match status {
                "up" => up += 1,
                "cheat" => cheat += 1,
                _ => down += 1,
            }
            info!(
                "Validator {} scored worker {}: status={} failure_code={} error={}",
                validator_uid, ip, status, failure_code, error
            );
        }

        let worker_list: Vec<Worker> = workers
            .iter()
            .filter_map(|w| serde_json::from_value(w.clone()).ok())
            .collect();

        if !worker_list.is_empty() {
            let _ = write_workers(&state.db, &worker_list, "internal", "");
            info!(
                "Updated {} workers from validator {} feedback ({} up, {} down, {} cheat)",
                worker_list.len(),
                validator_uid,
                up,
                down,
                cheat
            );
        }
    }

    Json(json!({"success": true}))
}

async fn persist_validator_score_feedback(state: &AppState, uid: &str, ip: &str, scores: Value) {
    let mut entries = match state.tpn_cache.get("dashboard_scores").await {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    };

    let received_at = chrono::Utc::now().to_rfc3339();
    let new_entry = json!({
        "uid": uid,
        "ip": ip,
        "received_at": received_at,
        "score": scores.get("score").cloned().unwrap_or(Value::Null),
        "stability_score": scores.get("stability_score").cloned().unwrap_or(Value::Null),
        "size_score": scores.get("size_score").cloned().unwrap_or(Value::Null),
        "performance_score": scores.get("performance_score").cloned().unwrap_or(Value::Null),
        "geo_score": scores.get("geo_score").cloned().unwrap_or(Value::Null),
    });

    if let Some(existing) = entries.iter_mut().find(|entry| {
        entry.get("uid").and_then(|value| value.as_str()) == Some(uid)
            && entry.get("ip").and_then(|value| value.as_str()) == Some(ip)
    }) {
        *existing = new_entry;
    } else {
        entries.push(new_entry);
    }

    state
        .tpn_cache
        .set("dashboard_scores", Value::Array(entries))
        .await;
}
