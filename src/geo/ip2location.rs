use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

const IP2LOCATION_DB_PATH: &str = "./ip2location_data/IP2LOCATION-LITE-ASN.BIN";
const UPDATE_INTERVAL_MS: i64 = 2 * 24 * 60 * 60 * 1000; // 2 days

/// Check if an IP appears to be from a datacenter based on ASN patterns.
/// This is a simplified check - the full implementation would read the BIN file.
pub fn is_datacenter_ip(_ip: &str) -> bool {
    // TODO: Implement full IP2Location BIN file reading
    // For now, return false (assume residential)
    false
}

/// Update IP2Location database if stale.
pub async fn update_ip2location(
    geo: &Arc<super::GeoService>,
    db: &crate::db::DbPool,
    token: Option<&str>,
) -> Result<()> {
    let last_update =
        crate::db::timestamps::get_timestamp(db, "last_ip2location_update").unwrap_or(0);
    let now = chrono::Utc::now().timestamp_millis();

    if now - last_update < UPDATE_INTERVAL_MS && Path::new(IP2LOCATION_DB_PATH).exists() {
        info!("IP2Location database is up to date");
        geo.set_ip2location_path(IP2LOCATION_DB_PATH).await;
        return Ok(());
    }

    let Some(token) = token else {
        if Path::new(IP2LOCATION_DB_PATH).exists() {
            geo.set_ip2location_path(IP2LOCATION_DB_PATH).await;
        } else {
            warn!("No IP2LOCATION_DOWNLOAD_TOKEN set and no existing database found");
        }
        return Ok(());
    };

    info!("Downloading IP2Location ASN database...");
    let url = format!(
        "https://www.ip2location.com/download/?token={}&file=DBASNLITEBINIPV6",
        token
    );

    match reqwest::get(&url).await {
        Ok(response) => {
            if response.status().is_success() {
                let bytes = response.bytes().await?;
                tokio::fs::create_dir_all("./ip2location_data").await?;
                let temp_path = "./ip2location_data/download.zip";
                tokio::fs::write(temp_path, &bytes).await?;

                // Extract using unzip
                let result = crate::system::shell::run(
                    &format!(
                        "cd ./ip2location_data && unzip -o download.zip '*.BIN' 2>/dev/null; rm -f download.zip"
                    ),
                    Some(30_000),
                )
                .await;

                if result.is_ok() {
                    geo.set_ip2location_path(IP2LOCATION_DB_PATH).await;
                    crate::db::timestamps::set_timestamp(db, "last_ip2location_update", now)?;
                    info!("IP2Location database updated successfully");
                } else {
                    warn!("Failed to extract IP2Location database");
                }
            } else {
                warn!("IP2Location download failed: {}", response.status());
            }
        }
        Err(e) => warn!("Failed to download IP2Location database: {}", e),
    }

    Ok(())
}
