use anyhow::{Context, Result};
use ip2location::Record;
#[cfg(feature = "embed-ip2location")]
use std::fs;
use std::fs::File;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

const IP2LOCATION_DATA_DIR: &str = "ip2location_data";
const IP2LOCATION_DB_FILE: &str = "IP2LOCATION-LITE-ASN.IPV6.BIN";
const IP2LOCATION_ZIP_FILE: &str = "ip2location.zip";
const UPDATE_INTERVAL_MS: i64 = 2 * 24 * 60 * 60 * 1000; // 2 days

#[cfg(feature = "embed-ip2location")]
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/ip2location_embedded.rs"));
}

/// Check if an IP appears to be from a datacenter based on IP2Location ASN/provider data.
pub fn is_datacenter_ip(db: &ip2location::DB, ip: IpAddr) -> bool {
    match db.ip_lookup(ip) {
        Ok(Record::LocationDb(record)) => {
            let mut fields = Vec::new();
            if let Some(as_name) = record.as_name.as_deref() {
                fields.push(as_name);
            }
            if let Some(isp) = record.isp.as_deref() {
                fields.push(isp);
            }
            if let Some(domain) = record.domain.as_deref() {
                fields.push(domain);
            }
            if let Some(usage_type) = record.as_usage_type.as_deref() {
                fields.push(usage_type);
            }

            fields.into_iter().any(matches_datacenter_pattern)
        }
        _ => false,
    }
}

/// Update IP2Location database if stale.
pub async fn update_ip2location(
    geo: &Arc<super::GeoService>,
    db: &crate::db::DbPool,
    config_dir: &Path,
    token: Option<&str>,
) -> Result<()> {
    #[cfg(feature = "embed-ip2location")]
    if let Some(path) = ensure_embedded_db(config_dir)? {
        geo.load_ip2location(&path.to_string_lossy()).await?;
        crate::db::timestamps::set_timestamp(
            db,
            "last_ip2location_update",
            chrono::Utc::now().timestamp_millis(),
        )?;
        return Ok(());
    }

    let data_dir = ip2location_data_dir(config_dir);
    let db_path = ip2location_db_path(config_dir);
    let last_update =
        crate::db::timestamps::get_timestamp(db, "last_ip2location_update").unwrap_or(0);
    let now = chrono::Utc::now().timestamp_millis();

    if now - last_update < UPDATE_INTERVAL_MS && db_path.exists() {
        info!("IP2Location database is up to date");
        geo.load_ip2location(&db_path.to_string_lossy()).await?;
        return Ok(());
    }

    let Some(token) = token else {
        if db_path.exists() {
            geo.load_ip2location(&db_path.to_string_lossy()).await?;
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
                tokio::fs::create_dir_all(&data_dir).await?;
                let temp_path = data_dir.join("download.zip");
                tokio::fs::write(&temp_path, &bytes).await?;

                let extracted = extract_ip2location_zip(&temp_path, &data_dir)?;
                let _ = tokio::fs::remove_file(&temp_path).await;

                if extracted && db_path.exists() {
                    geo.load_ip2location(&db_path.to_string_lossy()).await?;
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

fn ip2location_data_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(IP2LOCATION_DATA_DIR)
}

fn ip2location_db_path(config_dir: &Path) -> PathBuf {
    ip2location_data_dir(config_dir).join(IP2LOCATION_DB_FILE)
}

fn ip2location_zip_path(config_dir: &Path) -> PathBuf {
    ip2location_data_dir(config_dir).join(IP2LOCATION_ZIP_FILE)
}

fn matches_datacenter_pattern(value: &str) -> bool {
    let value = value.to_lowercase();
    super::DATACENTER_PATTERNS
        .iter()
        .any(|pattern| value.contains(pattern))
}

fn extract_ip2location_zip(zip_path: &Path, data_dir: &Path) -> Result<bool> {
    let file = File::open(zip_path)
        .with_context(|| format!("open IP2Location archive {}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .with_context(|| format!("read IP2Location archive {}", zip_path.display()))?;
    fs::create_dir_all(data_dir)
        .with_context(|| format!("create IP2Location directory {}", data_dir.display()))?;

    let mut extracted = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if !entry.name().ends_with(".BIN") {
            continue;
        }

        let Some(name) = Path::new(entry.name()).file_name() else {
            continue;
        };
        let output_path = data_dir.join(name);
        let mut output = File::create(&output_path)
            .with_context(|| format!("create extracted BIN {}", output_path.display()))?;
        std::io::copy(&mut entry, &mut output)
            .with_context(|| format!("extract BIN {}", output_path.display()))?;
        extracted = true;
    }

    Ok(extracted)
}

#[cfg(test)]
mod tests {
    use super::matches_datacenter_pattern;

    #[test]
    fn detects_known_datacenter_providers() {
        assert!(matches_datacenter_pattern("Amazon.com, Inc."));
        assert!(matches_datacenter_pattern("DigitalOcean, LLC"));
        assert!(matches_datacenter_pattern("Cloudflare, Inc."));
    }

    #[test]
    fn ignores_non_datacenter_names() {
        assert!(!matches_datacenter_pattern("Example Residential ISP"));
    }
}

#[cfg(feature = "embed-ip2location")]
fn ensure_embedded_db(config_dir: &Path) -> Result<Option<PathBuf>> {
    let target = ip2location_db_path(config_dir);
    if target.exists() {
        return Ok(Some(target));
    }
    let zip_path = ip2location_zip_path(config_dir);
    if let Some(parent) = zip_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if embedded::EMBED_IP2LOCATION_ZIP.is_empty() {
        return Ok(None);
    }
    fs::write(&zip_path, embedded::EMBED_IP2LOCATION_ZIP)?;
    let data_dir = ip2location_data_dir(config_dir);
    if extract_ip2location_zip(&zip_path, &data_dir)? && target.exists() {
        Ok(Some(target))
    } else {
        anyhow::bail!("failed to extract embedded IP2Location zip");
    }
}
