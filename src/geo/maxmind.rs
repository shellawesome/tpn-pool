use crate::db::timestamps;
use crate::db::DbPool;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

const MAXMIND_DB_PATH: &str = "./maxmind_data/GeoLite2-City.mmdb";
const UPDATE_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000; // 24 hours

/// Update MaxMind GeoLite2 database if stale.
pub async fn update_maxmind(
    geo: &Arc<super::GeoService>,
    db: &DbPool,
    license_key: Option<&str>,
) -> Result<()> {
    let last_update = timestamps::get_timestamp(db, "last_maxmind_update").unwrap_or(0);
    let now = chrono::Utc::now().timestamp_millis();

    if now - last_update < UPDATE_INTERVAL_MS && Path::new(MAXMIND_DB_PATH).exists() {
        info!("MaxMind database is up to date, loading existing file");
        geo.load_maxmind(MAXMIND_DB_PATH).await?;
        return Ok(());
    }

    let Some(key) = license_key else {
        if Path::new(MAXMIND_DB_PATH).exists() {
            geo.load_maxmind(MAXMIND_DB_PATH).await?;
        } else {
            warn!("No MAXMIND_LICENSE_KEY set and no existing database found");
        }
        return Ok(());
    };

    info!("Downloading MaxMind GeoLite2-City database...");
    let url = format!(
        "https://download.maxmind.com/app/geoip_download?edition_id=GeoLite2-City&license_key={}&suffix=tar.gz",
        key
    );

    match reqwest::get(&url).await {
        Ok(response) => {
            if response.status().is_success() {
                let bytes = response.bytes().await?;
                // Extract the .mmdb file from the tar.gz
                tokio::fs::create_dir_all("./maxmind_data").await?;
                let temp_path = "./maxmind_data/GeoLite2-City.tar.gz";
                tokio::fs::write(temp_path, &bytes).await?;

                // Use tar to extract
                let result = crate::system::shell::run(
                    &format!(
                        "cd ./maxmind_data && tar xzf GeoLite2-City.tar.gz --strip-components=1 '*/GeoLite2-City.mmdb' 2>/dev/null; rm -f GeoLite2-City.tar.gz"
                    ),
                    Some(30_000),
                )
                .await;

                if result.is_ok() && Path::new(MAXMIND_DB_PATH).exists() {
                    geo.load_maxmind(MAXMIND_DB_PATH).await?;
                    timestamps::set_timestamp(db, "last_maxmind_update", now)?;
                    info!("MaxMind database updated successfully");
                } else {
                    warn!("Failed to extract MaxMind database");
                    if Path::new(MAXMIND_DB_PATH).exists() {
                        geo.load_maxmind(MAXMIND_DB_PATH).await?;
                    }
                }
            } else {
                warn!("MaxMind download failed with status: {}", response.status());
            }
        }
        Err(e) => {
            warn!("Failed to download MaxMind database: {}", e);
            if Path::new(MAXMIND_DB_PATH).exists() {
                geo.load_maxmind(MAXMIND_DB_PATH).await?;
            }
        }
    }

    Ok(())
}
