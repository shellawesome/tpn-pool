use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tracing::info;

use crate::AppState;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

fn generate_token(secret: &str) -> Result<String, jsonwebtoken::errors::Error> {
    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::days(30))
        .expect("valid timestamp")
        .timestamp() as usize;

    let claims = Claims {
        sub: "admin".to_string(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
}

fn verify_token(token: &str, secret: &str) -> bool {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .is_ok()
}

/// Check if a request is authorized. Returns true if:
/// - No password is configured (auth disabled), OR
/// - A valid JWT is provided via Authorization header or ?token= query param.
pub fn verify_request(state: &AppState, headers: &HeaderMap, query: Option<&str>) -> bool {
    // No password set — skip auth entirely
    if state.config.login_password.is_empty() {
        return true;
    }

    let secret = &state.config.jwt_secret;

    // Check Authorization header
    if let Some(auth) = headers.get("Authorization").and_then(|h| h.to_str().ok()) {
        if let Some(token) = auth.strip_prefix("Bearer ") {
            if verify_token(token, secret) {
                return true;
            }
        }
    }

    // Check query param ?token=
    if let Some(q) = query {
        for part in q.split('&') {
            if let Some(value) = part.strip_prefix("token=") {
                let token = value.trim();
                if !token.is_empty() && verify_token(token, secret) {
                    return true;
                }
            }
        }
    }

    false
}

/// POST /api/login
pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    let expected = state.config.login_password.as_bytes();
    let provided = req.password.as_bytes();

    // Constant-time comparison
    let matches = if expected.len() == provided.len() {
        expected.ct_eq(provided).into()
    } else {
        false
    };

    if !matches {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "success": false, "message": "密码错误" })),
        );
    }

    match generate_token(&state.config.jwt_secret) {
        Ok(token) => (
            StatusCode::OK,
            Json(
                serde_json::json!({ "success": true, "message": "登录成功", "data": { "token": token } }),
            ),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "success": false, "message": "生成 token 失败" })),
        ),
    }
}

/// GET /api/auth/check — Public endpoint to check if auth is required
pub async fn auth_check(State(state): State<AppState>) -> impl IntoResponse {
    let required = !state.config.login_password.is_empty();
    Json(serde_json::json!({ "auth_required": required }))
}

/// GET /dashboard — Embedded HTML dashboard page.
pub async fn dashboard_page() -> Html<String> {
    info!("GET /dashboard requested — serving embedded HTML");
    Html(render_dashboard_html("dashboard", "/api/dashboard"))
}

/// GET /console — Embedded HTML console page.
pub async fn console_page() -> Html<String> {
    info!("GET /console requested — serving embedded HTML");
    Html(render_dashboard_html("console", "/api/console/dashboard"))
}

fn render_dashboard_html(mode: &str, api_path: &str) -> String {
    include_str!("dashboard.html")
        .replace("__TPN_MODE__", mode)
        .replace("__TPN_API_PATH__", api_path)
}
