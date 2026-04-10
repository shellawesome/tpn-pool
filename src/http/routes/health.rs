use crate::AppState;
use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Map, Value};

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(handler))
}

fn insert_if_some(map: &mut Map<String, Value>, key: &str, val: &Option<String>) {
    if let Some(v) = val {
        if !v.is_empty() {
            map.insert(key.to_string(), Value::String(v.clone()));
        }
    }
}

fn insert_if_not_empty(map: &mut Map<String, Value>, key: &str, val: &str) {
    if !val.is_empty() {
        map.insert(key.to_string(), Value::String(val.to_string()));
    }
}

async fn handler(State(state): State<AppState>) -> Json<Value> {
    let config = &state.config;
    let mut map = Map::new();

    let version = config
        .reported_version
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let branch = config
        .reported_branch
        .clone()
        .unwrap_or_else(|| state.branch.clone());
    let hash = config
        .reported_hash
        .clone()
        .unwrap_or_else(|| state.hash.clone());

    map.insert(
        "notice".to_string(),
        json!(format!(
            "I am a TPN Network miner component running v{}",
            version
        )),
    );
    map.insert("info".to_string(), json!("https://tpn.taofu.xyz"));
    map.insert("version".to_string(), json!(version));
    map.insert("branch".to_string(), json!(branch));
    map.insert("hash".to_string(), json!(hash));

    insert_if_some(&mut map, "MINING_POOL_NAME", &config.mining_pool_name);
    insert_if_some(&mut map, "MINING_POOL_URL", &config.mining_pool_url);
    insert_if_not_empty(&mut map, "SERVER_PUBLIC_HOST", &config.server_public_host);
    map.insert(
        "SERVER_PUBLIC_PORT".to_string(),
        json!(config.server_public_port.to_string()),
    );
    insert_if_not_empty(
        &mut map,
        "SERVER_PUBLIC_PROTOCOL",
        &config.server_public_protocol,
    );
    insert_if_some(&mut map, "MINING_POOL_REWARDS", &config.mining_pool_rewards);
    insert_if_some(
        &mut map,
        "MINING_POOL_WEBSITE_URL",
        &config.mining_pool_website_url,
    );
    insert_if_some(&mut map, "BROADCAST_MESSAGE", &config.broadcast_message);
    insert_if_some(&mut map, "CONTACT_METHOD", &config.contact_method);

    Json(Value::Object(map))
}
