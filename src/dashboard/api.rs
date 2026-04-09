use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json};
use serde_json::json;
use tracing::info;

use crate::db::workers::{get_workers, GetWorkersParams};
use crate::AppState;

use super::auth::verify_request;

/// GET /api/dashboard — Auth-gated aggregated dashboard JSON data.
pub async fn dashboard_data_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"}))).into_response();
    }

    info!("GET /api/dashboard requested");
    let config = &state.config;
    let uptime_secs = chrono::Utc::now()
        .signed_duration_since(state.start_time)
        .num_seconds();

    // Worker count and country distribution
    let (total_workers, workers_by_country) = match get_workers(
        &state.db,
        &GetWorkersParams {
            limit: Some(100000),
            ..Default::default()
        },
    ) {
        Ok(workers) => {
            let total = workers.len();
            let mut by_country: std::collections::HashMap<String, usize> =
                std::collections::HashMap::new();
            for w in &workers {
                *by_country.entry(w.country_code.clone()).or_default() += 1;
            }
            (total, by_country)
        }
        Err(_) => (0, std::collections::HashMap::new()),
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
            "internal_subnet": config.tpn_internal_subnet,
            "external_subnet": config.tpn_external_subnet,
        },
        "payment": {
            "evm_address": config.payment_address_evm,
            "bittensor_address": config.payment_address_bittensor,
        },
        "workers": {
            "total": total_workers,
            "up": up_workers,
            "by_country": workers_by_country,
        },
        "cache": {
            "keys": cache_keys,
        },
        "database": {
            "path": config.db_path,
        },
    }))
    .into_response()
}
