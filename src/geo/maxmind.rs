use crate::db::timestamps;
use crate::db::DbPool;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

const MAXMIND_CITY_DB_PATH: &str = "./maxmind_data/GeoLite2-City.mmdb";
const MAXMIND_ASN_DB_PATH: &str = "./maxmind_data/GeoLite2-ASN.mmdb";
const UPDATE_INTERVAL_MS: i64 = 24 * 60 * 60 * 1000; // 24 hours

/// Update MaxMind GeoLite2 databases (City + ASN) if stale.
pub async fn update_maxmind(
    geo: &Arc<super::GeoService>,
    db: &DbPool,
    license_key: Option<&str>,
) -> Result<()> {
    let last_update = timestamps::get_timestamp(db, "last_maxmind_update").unwrap_or(0);
    let now = chrono::Utc::now().timestamp_millis();

    if now - last_update < UPDATE_INTERVAL_MS
        && Path::new(MAXMIND_CITY_DB_PATH).exists()
        && Path::new(MAXMIND_ASN_DB_PATH).exists()
    {
        info!("MaxMind databases are up to date, loading existing files");
        geo.load_maxmind(MAXMIND_CITY_DB_PATH).await?;
        geo.load_maxmind_asn(MAXMIND_ASN_DB_PATH).await?;
        return Ok(());
    }

    let Some(key) = license_key else {
        if Path::new(MAXMIND_CITY_DB_PATH).exists() {
            geo.load_maxmind(MAXMIND_CITY_DB_PATH).await?;
        }
        if Path::new(MAXMIND_ASN_DB_PATH).exists() {
            geo.load_maxmind_asn(MAXMIND_ASN_DB_PATH).await?;
        }
        if !Path::new(MAXMIND_CITY_DB_PATH).exists() {
            warn!("No MAXMIND_LICENSE_KEY set and no existing database found");
        }
        return Ok(());
    };

    tokio::fs::create_dir_all("./maxmind_data").await?;

    // Download City and ASN databases in parallel
    let (city_result, asn_result) = tokio::join!(
        download_maxmind_db(key, "GeoLite2-City", MAXMIND_CITY_DB_PATH),
        download_maxmind_db(key, "GeoLite2-ASN", MAXMIND_ASN_DB_PATH),
    );

    let mut updated = false;

    match city_result {
        Ok(()) => {
            geo.load_maxmind(MAXMIND_CITY_DB_PATH).await?;
            updated = true;
        }
        Err(e) => {
            warn!("Failed to update MaxMind City database: {}", e);
            if Path::new(MAXMIND_CITY_DB_PATH).exists() {
                geo.load_maxmind(MAXMIND_CITY_DB_PATH).await?;
            }
        }
    }

    match asn_result {
        Ok(()) => {
            geo.load_maxmind_asn(MAXMIND_ASN_DB_PATH).await?;
            updated = true;
        }
        Err(e) => {
            warn!("Failed to update MaxMind ASN database: {}", e);
            if Path::new(MAXMIND_ASN_DB_PATH).exists() {
                geo.load_maxmind_asn(MAXMIND_ASN_DB_PATH).await?;
            }
        }
    }

    if updated {
        timestamps::set_timestamp(db, "last_maxmind_update", now)?;
        info!("MaxMind databases updated successfully");
    }

    Ok(())
}

async fn download_maxmind_db(key: &str, edition: &str, db_path: &str) -> Result<()> {
    info!("Downloading MaxMind {} database...", edition);
    let url = format!(
        "https://download.maxmind.com/app/geoip_download?edition_id={}&license_key={}&suffix=tar.gz",
        edition, key
    );

    let response = reqwest::get(&url).await?;
    if !response.status().is_success() {
        anyhow::bail!(
            "MaxMind {} download failed with status: {}",
            edition,
            response.status()
        );
    }

    let bytes = response.bytes().await?;
    let temp_path = format!("./maxmind_data/{}.tar.gz", edition);
    tokio::fs::write(&temp_path, &bytes).await?;

    let mmdb_name = format!("{}.mmdb", edition);
    let result = crate::system::shell::run(
        &format!(
            "cd ./maxmind_data && tar xzf {edition}.tar.gz --strip-components=1 '*/{mmdb_name}' 2>/dev/null; rm -f {edition}.tar.gz",
            edition = edition,
            mmdb_name = mmdb_name,
        ),
        Some(30_000),
    )
    .await;

    if result.is_ok() && Path::new(db_path).exists() {
        info!("MaxMind {} database extracted successfully", edition);
        Ok(())
    } else {
        anyhow::bail!("Failed to extract MaxMind {} database", edition)
    }
}
