use anyhow::Result;
use std::time::Duration;
use tracing::{debug, warn};

/// Minimum required version for TPN nodes.
const MIN_VERSION: &str = "1.3.0";

/// Check if a remote node is running an acceptable version.
pub async fn score_node_version(
    ip: &str,
    port: u16,
    public_url: Option<&str>,
) -> Result<(bool, String)> {
    let url = if let Some(purl) = public_url {
        purl.to_string()
    } else {
        format!("http://{}:{}", ip, port)
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    match client.get(&url).send().await {
        Ok(response) => {
            let body: serde_json::Value = response.json().await?;
            let version_str = body
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("0.0.0");

            let node_version = semver::Version::parse(version_str).unwrap_or(semver::Version::new(0, 0, 0));
            let min_version = semver::Version::parse(MIN_VERSION).unwrap();
            let valid = node_version >= min_version;

            if !valid {
                debug!(
                    "Node at {} running version {} (minimum: {})",
                    url, version_str, MIN_VERSION
                );
            }

            Ok((valid, version_str.to_string()))
        }
        Err(e) => {
            warn!("Failed to check version at {}: {}", url, e);
            Ok((false, "unknown".to_string()))
        }
    }
}
