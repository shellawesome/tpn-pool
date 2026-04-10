use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/stats", get(stats_handler))
        .route("/api/worker_performance", get(worker_performance_handler))
        .route("/api/request/:id", get(request_status_handler))
}

#[derive(Debug, Deserialize)]
struct StatsParams {
    api_key: Option<String>,
}

async fn stats_handler(
    State(state): State<AppState>,
    Query(params): Query<StatsParams>,
) -> Json<Value> {
    let config = &state.config;
    let country_count = state.cache.get_or("country_count", json!({}));
    let mut response = json!({
        "mode": "miner",
        "version": env!("CARGO_PKG_VERSION"),
        "base_url": config.base_url(),
        "country_count": country_count,
    });

    if let Some(ref admin_key) = state.config.admin_api_key {
        let authenticated = params.api_key.as_deref() == Some(admin_key.as_str());
        if authenticated {
            response["country_code_to_ips"] = state.cache.get_or("country_code_to_ips", json!({}));
        }
    }

    Json(response)
}

#[derive(Debug, Deserialize)]
struct PerformanceParams {
    api_key: Option<String>,
    from: Option<String>,
    to: Option<String>,
    history_days: Option<String>,
    format: Option<String>,
    group_by: Option<String>,
}

async fn worker_performance_handler(
    State(state): State<AppState>,
    Query(params): Query<PerformanceParams>,
) -> Response {
    // Validate API key
    let Some(ref admin_key) = state.config.admin_api_key else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "ADMIN_API_KEY not configured"})),
        )
            .into_response();
    };
    let provided = params.api_key.as_deref().unwrap_or("");
    if provided != admin_key {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "Invalid API key"})),
        )
            .into_response();
    }

    if params.history_days.is_some() && (params.from.is_some() || params.to.is_some()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Cannot specify both 'from'/'to' and 'history_days'"})),
        )
            .into_response();
    }

    let format = params.format.as_deref().unwrap_or("json");
    let group_by = params.group_by.as_deref().unwrap_or("ip");
    if !["json", "csv"].contains(&format) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid format"})),
        )
            .into_response();
    }
    if !["ip", "payment_address_evm", "payment_address_bittensor"].contains(&group_by) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "Invalid group_by"})),
        )
            .into_response();
    }

    let mut from = params.from.as_deref().and_then(parse_timestamp);
    let to = params.to.as_deref().and_then(parse_timestamp);
    if from.is_none() && to.is_none() {
        let history_days = params
            .history_days
            .as_deref()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(7);
        from = Some(chrono::Utc::now().timestamp_millis() - history_days * 24 * 60 * 60 * 1000);
    }

    let cache_key = format!(
        "worker_performance_{}_{}_{}_{}",
        group_by,
        from.unwrap_or_default(),
        to.unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        format
    );
    if let Some(cached) = state.cache.get(&cache_key) {
        return if format == "csv" {
            let csv_body = cached.as_str().unwrap_or_default().to_string();
            (StatusCode::OK, [("content-type", "text/csv")], csv_body).into_response()
        } else {
            (StatusCode::OK, Json(cached)).into_response()
        };
    }

    match crate::db::workers::get_worker_performance(&state.db, from, to) {
        Ok(records) => {
            let workers = crate::db::workers::get_workers(
                &state.db,
                &crate::db::workers::GetWorkersParams {
                    limit: Some(100_000),
                    ..Default::default()
                },
            )
            .unwrap_or_default();
            let mut metadata_by_ip: HashMap<String, crate::db::workers::Worker> = HashMap::new();
            for worker in workers {
                metadata_by_ip.entry(worker.ip.clone()).or_insert(worker);
            }

            let from_ts = from.unwrap_or(0);
            let to_ts = to.unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
            let response_json = build_worker_performance_response(
                records,
                &metadata_by_ip,
                from_ts,
                to_ts,
                group_by,
            );

            if format == "csv" {
                match json_to_csv(&response_json) {
                    Ok(csv_body) => {
                        state
                            .cache
                            .set(&cache_key, Value::String(csv_body.clone()), Some(300_000));
                        (StatusCode::OK, [("content-type", "text/csv")], csv_body).into_response()
                    }
                    Err(e) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": e.to_string()})),
                    )
                        .into_response(),
                }
            } else {
                state
                    .cache
                    .set(&cache_key, response_json.clone(), Some(300_000));
                (StatusCode::OK, Json(response_json)).into_response()
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

async fn request_status_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let request_key = format!("request_{}", id);
    let upstream_key = format!("request_upstream_{}", id);
    let checked_key = format!("request_upstream_checked_{}", id);

    let mut local_value = state.cache.get(&request_key).unwrap_or(Value::Null);

    if let Some(upstream) = state.cache.get(&upstream_key) {
        let recently_checked = state.cache.contains(&checked_key);
        if !recently_checked {
            state
                .cache
                .set(&checked_key, Value::Bool(true), Some(5_000));

            if let Some(url) = upstream.get("url").and_then(|v| v.as_str()) {
                if let Ok(resp) = reqwest::Client::new().get(url).send().await {
                    if let Ok(upstream_status) = resp.json::<Value>().await {
                        if upstream_status.get("status").and_then(|v| v.as_str())
                            == Some("complete")
                        {
                            let parsed = url::Url::parse(url);
                            let upstream_winner =
                                upstream_status.get("winner").and_then(|v| v.as_str());

                            if let Ok(parsed_url) = parsed {
                                let my_nonce = parsed_url
                                    .query_pairs()
                                    .find(|(k, _)| k == "nonce")
                                    .map(|(_, v)| v.to_string());
                                let pool_won = upstream_winner.is_none()
                                    || my_nonce.is_none()
                                    || upstream_winner == my_nonce.as_deref();

                                if !pool_won {
                                    local_value =
                                        json!({ "status": "complete", "winner": Value::Null });
                                    state.cache.set(
                                        &request_key,
                                        local_value.clone(),
                                        Some(60_000),
                                    );
                                }
                            } else if upstream_winner.is_some() {
                                local_value =
                                    json!({ "status": "complete", "winner": Value::Null });
                                state
                                    .cache
                                    .set(&request_key, local_value.clone(), Some(60_000));
                            }
                        }
                    }
                }
            }
        }
    }

    Json(if local_value.is_null() {
        json!({})
    } else {
        local_value
    })
}

fn parse_timestamp(value: &str) -> Option<i64> {
    value
        .parse::<i64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|dt| dt.timestamp_millis())
        })
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| dt.and_utc().timestamp_millis())
        })
}

fn build_worker_performance_response(
    records: Vec<(String, String, String, i64)>,
    metadata_by_ip: &HashMap<String, crate::db::workers::Worker>,
    from: i64,
    to: i64,
    group_by: &str,
) -> Value {
    let mut totals: HashMap<String, usize> = HashMap::from([
        ("up".to_string(), 0),
        ("down".to_string(), 0),
        ("unknown".to_string(), 0),
        ("cheat".to_string(), 0),
    ]);
    let mut by_ip: HashMap<String, serde_json::Map<String, Value>> = HashMap::new();

    for (ip, status, public_url, _ts) in records {
        *totals.entry(status.clone()).or_default() += 1;
        let entry = by_ip.entry(ip.clone()).or_insert_with(|| {
            let mut map = serde_json::Map::new();
            map.insert("ip".to_string(), Value::String(ip.clone()));
            map.insert("public_url".to_string(), Value::String(public_url.clone()));
            map.insert("from".to_string(), Value::Number(from.into()));
            map.insert("to".to_string(), Value::Number(to.into()));
            map.insert(
                "from_human".to_string(),
                Value::String(
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(from)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
            );
            map.insert(
                "to_human".to_string(),
                Value::String(
                    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(to)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
            );
            map.insert("up".to_string(), Value::Number(0.into()));
            map.insert("down".to_string(), Value::Number(0.into()));
            map.insert("unknown".to_string(), Value::Number(0.into()));
            map.insert("cheat".to_string(), Value::Number(0.into()));
            map.insert("uptime".to_string(), json!(0.0));
            if let Some(worker) = metadata_by_ip.get(&ip) {
                if let Some(addr) = &worker.payment_address_evm {
                    map.insert(
                        "payment_address_evm".to_string(),
                        Value::String(addr.clone()),
                    );
                }
                if let Some(addr) = &worker.payment_address_bittensor {
                    map.insert(
                        "payment_address_bittensor".to_string(),
                        Value::String(addr.clone()),
                    );
                }
            }
            map
        });

        let current = entry.get(&status).and_then(|v| v.as_u64()).unwrap_or(0);
        entry.insert(status, Value::Number((current + 1).into()));
        let up = entry.get("up").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let down = entry.get("down").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let unknown = entry.get("unknown").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cheat = entry.get("cheat").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let total = up + down + unknown + cheat;
        let uptime = if total > 0.0 {
            (up / total * 10000.0).round() / 100.0
        } else {
            0.0
        };
        entry.insert("uptime".to_string(), json!(uptime));
    }

    let total_up = totals.get("up").copied().unwrap_or(0) as f64;
    let mut workers: Vec<Value> = by_ip
        .into_values()
        .map(|mut map| {
            let up = map.get("up").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let payment_fraction = if total_up > 0.0 {
                ((up / total_up) * 10000.0).round() / 10000.0
            } else {
                0.0
            };
            map.insert("payment_fraction".to_string(), json!(payment_fraction));
            Value::Object(map)
        })
        .collect();

    workers.sort_by(|a, b| {
        let a_uptime = a.get("uptime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b_uptime = b.get("uptime").and_then(|v| v.as_f64()).unwrap_or(0.0);
        b_uptime
            .partial_cmp(&a_uptime)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    match group_by {
        "payment_address_evm" | "payment_address_bittensor" => {
            let mut grouped: HashMap<String, f64> = HashMap::new();
            for worker in &workers {
                if let Some(key) = worker.get(group_by).and_then(|v| v.as_str()) {
                    if !key.is_empty() {
                        let fraction = worker
                            .get("payment_fraction")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        *grouped.entry(key.to_string()).or_default() += fraction;
                    }
                }
            }
            let mut rows: Vec<Value> = grouped
                .into_iter()
                .map(|(key, payment_fraction)| json!({ group_by: key, "payment_fraction": payment_fraction }))
                .collect();
            rows.sort_by(|a, b| {
                let a_val = a
                    .get("payment_fraction")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let b_val = b
                    .get("payment_fraction")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                b_val
                    .partial_cmp(&a_val)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Value::Array(rows)
        }
        _ => Value::Array(workers),
    }
}

fn json_to_csv(value: &Value) -> anyhow::Result<String> {
    let Value::Array(rows) = value else {
        return Ok(String::new());
    };
    let mut writer = csv::Writer::from_writer(vec![]);
    let headers: Vec<String> = rows
        .iter()
        .filter_map(|row| row.as_object())
        .flat_map(|obj| obj.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if headers.is_empty() {
        return Ok(String::new());
    }

    writer.write_record(&headers)?;
    for row in rows {
        if let Some(obj) = row.as_object() {
            let record: Vec<String> = headers
                .iter()
                .map(|key| match obj.get(key) {
                    Some(Value::Null) | None => String::new(),
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                })
                .collect();
            writer.write_record(&record)?;
        }
    }
    let bytes = writer.into_inner()?;
    Ok(String::from_utf8(bytes)?)
}
