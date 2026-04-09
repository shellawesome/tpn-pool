use axum::{
    extract::{ConnectInfo, State},
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use tracing::info;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/protocol/broadcast/neurons", post(handler))
}

#[derive(Debug, Deserialize)]
struct NeuronBroadcast {
    neurons: Option<Vec<Neuron>>,
}

#[derive(Debug, Deserialize)]
struct Neuron {
    uid: Option<Value>,
    ip: Option<String>,
    validator_trust: Option<f64>,
}

async fn handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(payload): Json<NeuronBroadcast>,
) -> Json<Value> {
    if !crate::networking::network::is_local_request(&addr) {
        return Json(json!({"error": "Request not from localhost"}));
    }

    let neurons = payload.neurons.unwrap_or_default();
    info!("Received neuron broadcast with {} neurons", neurons.len());

    let mut validators = Vec::new();
    let mut country_count = serde_json::Map::new();
    let mut country_code_to_ips = serde_json::Map::new();

    for neuron in &neurons {
        let uid = neuron
            .uid
            .as_ref()
            .map(|v| match v {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => v.to_string(),
            })
            .unwrap_or_default();
        let ip = neuron.ip.as_deref().unwrap_or("");
        let trust = neuron.validator_trust.unwrap_or(0.0);

        if ip.is_empty() || uid.is_empty() {
            continue;
        }

        if trust > 0.0 {
            validators.push(json!({"uid": uid, "ip": ip, "validator_trust": trust}));
        } else {
            let geodata = state.geo.lookup(ip).await;
            let country_code = if geodata.country_code.is_empty() {
                "XX".to_string()
            } else {
                geodata.country_code
            };
            let count = country_count
                .get(&country_code)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            country_count.insert(country_code.clone(), Value::Number((count + 1).into()));

            let ips = country_code_to_ips
                .entry(country_code)
                .or_insert_with(|| Value::Array(vec![]));
            if let Value::Array(arr) = ips {
                arr.push(Value::String(ip.to_string()));
            }
        }
    }

    // Miner mode only needs the current validator list from the neuron broadcast.
    state
        .cache
        .set_permanent("last_known_validators", Value::Array(validators.clone()));
    state
        .cache
        .set_permanent("country_count", Value::Object(country_count));
    state
        .cache
        .set_permanent("country_code_to_ips", Value::Object(country_code_to_ips));

    info!("Updated neuron cache with {} validators", validators.len());

    Json(json!({"success": true}))
}
