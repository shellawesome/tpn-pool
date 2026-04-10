use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::AppState;

use super::auth::verify_request;

const REPO: &str = "shellawesome/tpn-pool";
const ASSET_NAME: &str = "tpn-pool-linux-ubuntu22-amd64";

#[derive(Serialize)]
struct VersionInfo {
    current: String,
    latest: Option<String>,
    has_update: bool,
    download_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "unauthorized" })),
    )
        .into_response()
}

pub async fn get_version(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return unauthorized();
    }

    let current = format!("v{}", env!("CARGO_PKG_VERSION"));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => {
            return Json(serde_json::json!({
                "success": true,
                "data": VersionInfo { current, latest: None, has_update: false, download_url: None }
            }))
            .into_response();
        }
    };

    let url = format!("https://api.github.com/repos/{}/releases/latest", REPO);
    let resp = client.get(url).header("User-Agent", "tpn-pool").send().await;

    match resp {
        Ok(r) => match r.json::<GitHubRelease>().await {
            Ok(release) => {
                let latest = release.tag_name.clone();
                let has_update = latest.trim_start_matches('v') != current.trim_start_matches('v');
                let download_url = release
                    .assets
                    .iter()
                    .find(|asset| asset.name == ASSET_NAME)
                    .map(|asset| asset.browser_download_url.clone());

                Json(serde_json::json!({
                    "success": true,
                    "data": VersionInfo { current, latest: Some(latest), has_update, download_url }
                }))
                .into_response()
            }
            Err(_) => Json(serde_json::json!({
                "success": true,
                "data": VersionInfo { current, latest: None, has_update: false, download_url: None }
            }))
            .into_response(),
        },
        Err(_) => Json(serde_json::json!({
            "success": true,
            "data": VersionInfo { current, latest: None, has_update: false, download_url: None }
        }))
        .into_response(),
    }
}

pub async fn do_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return unauthorized();
    }

    let download_url = format!(
        "https://github.com/{}/releases/download/latest/{}",
        REPO, ASSET_NAME
    );

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("failed to create http client: {}", e)
                })),
            )
                .into_response();
        }
    };

    let binary_data = match client
        .get(&download_url)
        .header("User-Agent", "tpn-pool")
        .send()
        .await
    {
        Ok(r) => {
            if !r.status().is_success() {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "success": false,
                        "message": format!("download failed, HTTP {}", r.status())
                    })),
                )
                    .into_response();
            }
            match r.bytes().await {
                Ok(bytes) => bytes,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "success": false,
                            "message": format!("failed to read download body: {}", e)
                        })),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("download failed: {}", e)
                })),
            )
                .into_response();
        }
    };

    let temp_path = "/tmp/tpn-pool-new";
    if let Err(e) = std::fs::write(temp_path, &binary_data) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("failed to write temp file: {}", e)
            })),
        )
            .into_response();
    }

    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(temp_path, std::fs::Permissions::from_mode(0o755)) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("failed to chmod temp file: {}", e)
            })),
        )
            .into_response();
    }

    let verify = tokio::process::Command::new(temp_path)
        .arg("--help")
        .output()
        .await;
    if verify.is_err() {
        let _ = std::fs::remove_file(temp_path);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": "new binary verification failed"
            })),
        )
            .into_response();
    }

    let current_exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("failed to get current binary path: {}", e)
                })),
            )
                .into_response();
        }
    };

    let backup_path = format!("{}.bak", current_exe.display());
    if let Err(e) = std::fs::copy(&current_exe, &backup_path) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("backup failed: {}", e)
            })),
        )
            .into_response();
    }

    if let Err(e) = std::fs::remove_file(&current_exe) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("failed to remove old binary: {}", e)
            })),
        )
            .into_response();
    }

    if let Err(e) = std::fs::copy(temp_path, &current_exe) {
        let _ = std::fs::copy(&backup_path, &current_exe);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("failed to replace binary: {}", e)
            })),
        )
            .into_response();
    }

    if let Err(e) =
        std::fs::set_permissions(&current_exe, std::fs::Permissions::from_mode(0o755))
    {
        let _ = std::fs::remove_file(&current_exe);
        let _ = std::fs::copy(&backup_path, &current_exe);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "success": false,
                "message": format!("failed to chmod new binary: {}", e)
            })),
        )
            .into_response();
    }

    let _ = std::fs::remove_file(temp_path);

    let current_exe_string = current_exe.display().to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = tokio::process::Command::new("nohup")
            .arg(current_exe_string)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        std::process::exit(0);
    });

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "message": "upgrade successful, restarting..."
        })),
    )
        .into_response()
}

pub async fn stop_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return unauthorized();
    }

    tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        std::process::exit(0);
    });

    Json(serde_json::json!({
        "success": true,
        "message": "stopping service..."
    }))
    .into_response()
}

pub async fn restart_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> impl IntoResponse {
    if !verify_request(&state, &headers, uri.query()) {
        return unauthorized();
    }

    let pid = std::process::id();
    let current_exe = match std::env::current_exe() {
        Ok(path) => path.display().to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("failed to get current binary path: {}", e)
                })),
            )
                .into_response();
        }
    };

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let script = format!(
            "sleep 1; while kill -0 {} 2>/dev/null; do sleep 0.2; done; nohup {} > /dev/null 2>&1 &",
            pid, current_exe
        );
        let _ = std::process::Command::new("sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        std::process::exit(0);
    });

    Json(serde_json::json!({
        "success": true,
        "message": "restarting service..."
    }))
    .into_response()
}
