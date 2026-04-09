use axum::{
    extract::{ConnectInfo, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;

use crate::db::workers::get_worker_countries_for_pool;
use crate::geo::helpers::country_name_from_code;
use crate::networking::network::ip_from_request;
use crate::networking::validators::is_validator_request;
use crate::validations::sanitize_string;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/lease/new", get(lease_handler))
        .route("/api/config/new", get(lease_handler))
        .route("/api/lease/countries", get(countries_handler))
        .route("/api/config/countries", get(countries_handler))
}

#[derive(Debug, Deserialize)]
struct LeaseParams {
    lease_seconds: Option<String>,
    lease_minutes: Option<String>,
    format: Option<String>,
    geo: Option<String>,
    whitelist: Option<String>,
    blacklist: Option<String>,
    #[serde(rename = "type")]
    config_type: Option<String>,
    connection_type: Option<String>,
    feedback_url: Option<String>,
}

async fn lease_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(params): Query<LeaseParams>,
) -> impl IntoResponse {
    let config = state.config.clone();
    let db = state.db.clone();
    let cache = state.cache.clone();
    let unspoofable_ip = ip_from_request(&addr);

    // Only accept lease requests from validators
    let is_val = is_validator_request(&unspoofable_ip, &cache);
    if is_val.is_none() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": format!("Only accept lease requests from validators, which you ({}) are not", unspoofable_ip)})),
        )
            .into_response();
    }

    // Parse params
    let mut lease_seconds: i64 = params
        .lease_seconds
        .as_deref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    if lease_seconds == 0 {
        if let Some(ref minutes) = params.lease_minutes {
            lease_seconds = minutes.parse::<i64>().unwrap_or(0) * 60;
        }
    }

    if lease_seconds <= 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid lease_seconds"})),
        )
            .into_response();
    }

    let geo = params.geo.as_deref().unwrap_or("ANY").to_uppercase();
    let config_type = params
        .config_type
        .clone()
        .unwrap_or_else(|| "wireguard".to_string());
    let response_format = params
        .format
        .clone()
        .unwrap_or_else(|| "json".to_string());
    let connection_type = params
        .connection_type
        .clone()
        .unwrap_or_else(|| "any".to_string());
    let priority = true;
    let feedback_url = params.feedback_url.clone();

    let whitelist: Option<Vec<String>> = params
        .whitelist
        .as_deref()
        .map(|s| s.split(',').map(|ip| sanitize_string(ip.trim())).collect());
    let blacklist: Option<Vec<String>> = params
        .blacklist
        .as_deref()
        .map(|s| s.split(',').map(|ip| sanitize_string(ip.trim())).collect());

    // Validate config_type
    if !["wireguard", "socks5"].contains(&config_type.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Invalid type: {}", config_type)})),
        )
            .into_response();
    }
    if !["json", "text"].contains(&response_format.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Invalid format: {}", response_format)})),
        )
            .into_response();
    }

    let result = crate::api::mining_pool::get_worker_config_as_miner(
        &db,
        &config,
        &cache,
        lease_seconds,
        &geo,
        &config_type,
        whitelist.as_deref(),
        blacklist.as_deref(),
        priority,
        &response_format,
        &connection_type,
        feedback_url.as_deref(),
    )
    .await;

    match result {
        Ok(data) => {
            let mut response: Response = match data.body {
                crate::api::mining_pool::LeaseResponseBody::Json(body) => {
                    (StatusCode::OK, Json(body)).into_response()
                }
                crate::api::mining_pool::LeaseResponseBody::Text(body) => {
                    (StatusCode::OK, body).into_response()
                }
            };

            if let Some(lease_ref) = data.lease_ref {
                if let Ok(value) = lease_ref.parse() {
                    response.headers_mut().insert("X-Lease-Ref", value);
                }
            }
            if let Some(lease_expires_at) = data.lease_expires_at {
                if let Ok(value) = lease_expires_at.to_string().parse() {
                    response.headers_mut().insert("X-Lease-Expires", value);
                }
            }

            response
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Error handling lease: {}", e)})),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
struct CountriesParams {
    format: Option<String>,
    #[serde(rename = "type")]
    name_type: Option<String>,
    connection_type: Option<String>,
}

async fn countries_handler(
    State(state): State<AppState>,
    Query(params): Query<CountriesParams>,
) -> impl IntoResponse {
    let format = params.format.as_deref().unwrap_or("json");
    let name_type = params.name_type.as_deref().unwrap_or("code");
    let connection_type = params.connection_type.as_deref().unwrap_or("any");

    match get_worker_countries_for_pool(&state.db, None, Some(connection_type)) {
        Ok(codes) => {
            if name_type == "name" {
                let names: Vec<&str> = codes.iter().map(|c| country_name_from_code(c)).collect();
                if format == "text" {
                    (StatusCode::OK, names.join("\n")).into_response()
                } else {
                    (StatusCode::OK, Json(json!(names))).into_response()
                }
            } else if format == "text" {
                (StatusCode::OK, codes.join("\n")).into_response()
            } else {
                (StatusCode::OK, Json(json!(codes))).into_response()
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
