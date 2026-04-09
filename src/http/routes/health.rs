use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(handler))
}

async fn handler(State(state): State<AppState>) -> Json<Value> {
    let config = &state.config;
    let last_start = state.start_time.to_rfc3339();
    Json(json!({
        "notice": format!("I am a TPN Network miner component running v{}", env!("CARGO_PKG_VERSION")),
        "info": "https://tpn.taofu.xyz",
        "mode": "miner",
        "version": env!("CARGO_PKG_VERSION"),
        "last_start": last_start,
        "branch": state.branch,
        "hash": state.hash,
        "MINING_POOL_URL": config.mining_pool_url,
        "MINING_POOL_NAME": config.mining_pool_name,
        "MINING_POOL_WEBSITE_URL": config.mining_pool_website_url,
        "MINING_POOL_REWARDS": config.mining_pool_rewards,
        "SERVER_PUBLIC_PROTOCOL": config.server_public_protocol,
        "SERVER_PUBLIC_HOST": config.server_public_host,
        "SERVER_PUBLIC_PORT": config.server_public_port,
    }))
}
