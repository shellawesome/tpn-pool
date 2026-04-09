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
use crate::networking::validators::is_validator_request;
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
        public_url: payload.get("public_url").and_then(|v| v.as_str()).map(|s| s.to_string()),
        payment_address_evm: payload.get("payment_address_evm").and_then(|v| v.as_str()).map(|s| s.to_string()),
        payment_address_bittensor: payload.get("payment_address_bittensor").and_then(|v| v.as_str()).map(|s| s.to_string()),
        public_port: payload.get("public_port").and_then(|v| v.as_str().or_else(|| v.as_i64().map(|_| ""))).unwrap_or("3000").to_string(),
        mining_pool_url: payload.get("mining_pool_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        mining_pool_uid: "internal".to_string(),
        status: "unknown".to_string(),
        country_code: "XX".to_string(),
        connection_type: "unknown".to_string(),
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
    let geodata = state.geo.lookup(&unspoofable_ip).await;
    worker.country_code = geodata.country_code;
    worker.connection_type = geodata.connection_type;

    // Check for IP clashes
    let clashes = find_clashing_workers(&state.db, &[worker.clone()], "internal").unwrap_or_default();
    if !clashes.is_empty() {
        warn!("IP clash detected for {}: {} existing workers", unspoofable_ip, clashes.len());
    }

    // Set status to up
    worker.status = "up".to_string();

    // Save worker
    match write_workers(&state.db, &[worker.clone()], "internal", "") {
        Ok(()) => {
            info!("Registered worker {} from {}", unspoofable_ip, worker.country_code);
            Json(json!({
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
    if validator.is_none() {
        return Json(json!({"success": false, "error": "Forbidden, endpoint only for validators"}));
    }

    info!("Received worker feedback from validator");

    // Process composite scores if present
    if let Some(scores) = payload.get("composite_scores") {
        info!("Validator composite scores: {:?}", scores);
    }

    // Process worker status updates
    if let Some(workers) = payload.get("workers_with_status").and_then(|v| v.as_array()) {
        let worker_list: Vec<Worker> = workers
            .iter()
            .filter_map(|w| serde_json::from_value(w.clone()).ok())
            .collect();

        if !worker_list.is_empty() {
            let _ = write_workers(&state.db, &worker_list, "internal", "");
            info!("Updated {} workers from validator feedback", worker_list.len());
        }
    }

    Json(json!({"success": true}))
}
