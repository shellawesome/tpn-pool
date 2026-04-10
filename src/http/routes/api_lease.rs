use axum::{
    extract::{ConnectInfo, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::net::Ipv4Addr;
use std::net::SocketAddr;

use crate::crypto::lease_token::{sign_lease_token, verify_lease_token, LeaseTokenPayload};
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
    lease_token: Option<String>,
    extend_ref: Option<String>,
    extend_expires_at: Option<String>,
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
    let response_format = params.format.clone().unwrap_or_else(|| "json".to_string());
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
    if !["any", "datacenter", "residential"].contains(&connection_type.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("Invalid connection_type: {}", connection_type)})),
        )
            .into_response();
    }
    if whitelist
        .as_ref()
        .is_some_and(|ips| ips.iter().any(|ip| !is_valid_ipv4(ip)))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid ip addresses in whitelist"})),
        )
            .into_response();
    }
    if blacklist
        .as_ref()
        .is_some_and(|ips| ips.iter().any(|ip| !is_valid_ipv4(ip)))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid ip addresses in blacklist"})),
        )
            .into_response();
    }

    let available_countries =
        get_worker_countries_for_pool(&db, Some("internal"), Some(&connection_type))
            .unwrap_or_default();
    if geo != "ANY" && !available_countries.iter().any(|country| country == &geo) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("No workers found for geo: {}", geo)})),
        )
            .into_response();
    }

    // Validate extension params: lease_token and extend_ref are mutually exclusive
    if params.lease_token.is_some() && params.extend_ref.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Ambiguous extension request: provide either lease_token or extend_ref, not both"})),
        )
            .into_response();
    }
    if params.extend_ref.is_some() && params.extend_expires_at.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "extend_expires_at is required when extend_ref is provided"})),
        )
            .into_response();
    }

    // Resolve extension parameters from lease_token if provided
    let (extend_ref, extend_expires_at, pinned_worker_ip) = if let Some(ref token) =
        params.lease_token
    {
        let Some(secret) = config
            .lease_token_secret
            .as_deref()
            .filter(|secret| !secret.is_empty())
        else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "lease_token extensions are disabled: LEASE_TOKEN_SECRET is not configured"})),
            )
                .into_response();
        };

        match verify_lease_token(secret, token) {
            Ok(payload) => (
                Some(payload.config_ref),
                Some(payload.expires_at.to_string()),
                Some(payload.worker_ip),
            ),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("Invalid lease_token: {}", e)})),
                )
                    .into_response();
            }
        }
    } else {
        (
            params.extend_ref.clone(),
            params.extend_expires_at.clone(),
            None,
        )
    };

    let result = crate::api::mining_pool::get_worker_config_as_miner(
        &crate::api::mining_pool::LeaseRequestParams {
            pool: &db,
            config: &config,
            cache: &cache,
            lease_seconds,
            pinned_worker_ip: pinned_worker_ip.as_deref(),
            geo: &geo,
            config_type: &config_type,
            whitelist: whitelist.as_deref(),
            blacklist: blacklist.as_deref(),
            priority,
            response_format: &response_format,
            connection_type: &connection_type,
            feedback_url: feedback_url.as_deref(),
            extend_ref: extend_ref.as_deref(),
            extend_expires_at: extend_expires_at.as_deref(),
        },
    )
    .await;

    match result {
        Ok(data) => {
            // Sign a lease extension token for the response
            let lease_token =
                if let (Some(ref lease_ref), Some(lease_expires), Some(ref worker_ip)) =
                    (&data.lease_ref, data.lease_expires_at, &data.worker_ip)
                {
                    let secret = config.lease_token_secret.as_deref().unwrap_or("");
                    if !secret.is_empty() {
                        let payload = LeaseTokenPayload {
                            config_ref: lease_ref.clone(),
                            lease_type: config_type.clone(),
                            worker_ip: worker_ip.clone(),
                            mining_pool_url: config.base_url(),
                            mining_pool_uid: "internal".to_string(),
                            expires_at: lease_expires,
                        };
                        Some(sign_lease_token(secret, &payload))
                    } else {
                        None
                    }
                } else {
                    None
                };

            let mut response: Response = match data.body {
                crate::api::mining_pool::LeaseResponseBody::Json(mut body) => {
                    // Enrich JSON body with metadata
                    if let Some(ref country) = data.country {
                        if body.get("country").is_none() {
                            body["country"] = json!(country);
                        }
                    }
                    if let Some(ref ct) = data.connection_type {
                        if body.get("connection_type").is_none() {
                            body["connection_type"] = json!(ct);
                        }
                    }
                    if let Some(ref token) = lease_token {
                        body["lease_token"] = json!(token);
                    }
                    (StatusCode::OK, Json(body)).into_response()
                }
                crate::api::mining_pool::LeaseResponseBody::Text(body) => {
                    (StatusCode::OK, body).into_response()
                }
            };

            // Set response headers
            let headers = response.headers_mut();
            if let Some(ref country) = data.country {
                if let Ok(value) = country.parse() {
                    headers.insert("X-Country", value);
                }
            }
            if let Some(ref ct) = data.connection_type {
                if let Ok(value) = ct.parse() {
                    headers.insert("X-Connection-Type", value);
                }
            }
            if let Some(lease_ref) = data.lease_ref {
                if let Ok(value) = lease_ref.parse() {
                    headers.insert("X-Lease-Ref", value);
                }
            }
            if let Some(lease_expires_at) = data.lease_expires_at {
                if let Ok(value) = lease_expires_at.to_string().parse() {
                    headers.insert("X-Lease-Expires", value);
                }
            }
            if let Some(token) = lease_token {
                if let Ok(value) = token.parse() {
                    headers.insert("X-Lease-Extension-Token", value);
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

fn is_valid_ipv4(ip: &str) -> bool {
    ip.parse::<Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::tpn_cache::TpnCache;
    use crate::cache::TtlCache;
    use crate::config::AppConfig;
    use crate::db;
    use crate::geo::GeoService;
    use crate::locks::LockRegistry;
    use crate::AppState;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn test_config(db_path: String) -> AppConfig {
        AppConfig {
            config_dir: std::env::temp_dir(),
            env_file_path: std::env::temp_dir().join("tpn-pool-test.env"),
            python_shim_path: std::env::temp_dir().join("miner_shim.py"),
            server_port: 3000,
            server_public_protocol: "http".to_string(),
            server_public_host: "localhost".to_string(),
            server_public_port: 3000,
            db_path,
            force_destroy_database: true,
            ci_mode: true,
            ci_mock_mining_pool_responses: false,
            maxmind_license_key: None,
            ip2location_download_token: None,
            lease_token_secret: None,
            admin_api_key: None,
            mining_pool_url: None,
            mining_pool_name: None,
            mining_pool_website_url: None,
            mining_pool_rewards: None,
            payment_address_evm: None,
            payment_address_bittensor: None,
            tpn_internal_subnet: "10.13.13.0/24".to_string(),
            tpn_external_subnet: "10.14.14.0/24".to_string(),
            daemon_interval_seconds: 60,
            force_refresh: false,
            partnered_network_mining_pools: vec![],
            log_level: "info".to_string(),
            login_password: String::new(),
            jwt_secret: "test-secret".to_string(),
            python_shim_enabled: false,
            python_bin: "python3".to_string(),
            tpn_subnet_python_root: None,
            bt_netuid: None,
            bt_subtensor_network: "finney".to_string(),
            bt_subtensor_chain_endpoint: None,
            bt_hotkey_mnemonic: None,
            bt_hotkey_seed_hex: None,
            bt_coldkey_mnemonic: None,
            bt_coldkey_seed_hex: None,
            bt_axon_port: 8091,
            bt_external_ip: None,
            bt_force_validator_permit: true,
            bt_allow_non_registered: false,
            python_shim_restart_delay_seconds: 5,
        }
    }

    fn test_state() -> AppState {
        let db_path =
            std::env::temp_dir().join(format!("tpn-pool-test-{}.db", uuid::Uuid::new_v4()));
        let config = test_config(path_to_string(db_path));
        let db = db::init_pool(&config).expect("db init");
        db::init_schema(&db, &config).expect("schema init");
        let cache = Arc::new(TtlCache::new());
        cache.set_permanent(
            "last_known_validators",
            serde_json::json!([{ "uid": "validator-1", "ip": "127.0.0.1" }]),
        );

        AppState {
            db,
            cache,
            tpn_cache: Arc::new(TpnCache::new("/tmp/tpn-pool-test-cache.json")),
            config,
            locks: Arc::new(LockRegistry::new()),
            geo: Arc::new(GeoService::new()),
            branch: "test".to_string(),
            hash: "test".to_string(),
            start_time: chrono::Utc::now(),
        }
    }

    fn path_to_string(path: PathBuf) -> String {
        path.to_string_lossy().into_owned()
    }

    async fn response_json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn rejects_invalid_connection_type() {
        let state = test_state();
        let response = lease_handler(
            axum::extract::State(state),
            axum::extract::ConnectInfo("127.0.0.1:9999".parse().unwrap()),
            axum::extract::Query(LeaseParams {
                lease_seconds: Some("60".to_string()),
                lease_minutes: None,
                format: Some("json".to_string()),
                geo: Some("ANY".to_string()),
                whitelist: None,
                blacklist: None,
                config_type: Some("wireguard".to_string()),
                connection_type: Some("invalid".to_string()),
                feedback_url: None,
                lease_token: None,
                extend_ref: None,
                extend_expires_at: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"], "Invalid connection_type: invalid");
    }

    #[tokio::test]
    async fn rejects_invalid_whitelist_ip() {
        let state = test_state();
        let response = lease_handler(
            axum::extract::State(state),
            axum::extract::ConnectInfo("127.0.0.1:9999".parse().unwrap()),
            axum::extract::Query(LeaseParams {
                lease_seconds: Some("60".to_string()),
                lease_minutes: None,
                format: Some("json".to_string()),
                geo: Some("ANY".to_string()),
                whitelist: Some("1.2.3.999".to_string()),
                blacklist: None,
                config_type: Some("wireguard".to_string()),
                connection_type: Some("any".to_string()),
                feedback_url: None,
                lease_token: None,
                extend_ref: None,
                extend_expires_at: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"], "Invalid ip addresses in whitelist");
    }

    #[tokio::test]
    async fn rejects_unavailable_geo() {
        let state = test_state();
        let response = lease_handler(
            axum::extract::State(state),
            axum::extract::ConnectInfo("127.0.0.1:9999".parse().unwrap()),
            axum::extract::Query(LeaseParams {
                lease_seconds: Some("60".to_string()),
                lease_minutes: None,
                format: Some("json".to_string()),
                geo: Some("NL".to_string()),
                whitelist: None,
                blacklist: None,
                config_type: Some("wireguard".to_string()),
                connection_type: Some("any".to_string()),
                feedback_url: None,
                lease_token: None,
                extend_ref: None,
                extend_expires_at: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["error"], "No workers found for geo: NL");
    }

    #[tokio::test]
    async fn rejects_lease_token_when_secret_unset() {
        let state = test_state();
        let response = lease_handler(
            axum::extract::State(state),
            axum::extract::ConnectInfo("127.0.0.1:9999".parse().unwrap()),
            axum::extract::Query(LeaseParams {
                lease_seconds: Some("60".to_string()),
                lease_minutes: None,
                format: Some("json".to_string()),
                geo: Some("ANY".to_string()),
                whitelist: None,
                blacklist: None,
                config_type: Some("wireguard".to_string()),
                connection_type: Some("any".to_string()),
                feedback_url: None,
                lease_token: Some("opaque-token".to_string()),
                extend_ref: None,
                extend_expires_at: None,
            }),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(
            body["error"],
            "lease_token extensions are disabled: LEASE_TOKEN_SECRET is not configured"
        );
    }
}
