pub mod helpers;
pub mod ip2location;
pub mod maxmind;

use anyhow::Result;
use std::net::IpAddr;
use tokio::sync::RwLock;
use tracing::info;

/// Unified geolocation service combining MaxMind and IP2Location.
pub struct GeoService {
    maxmind_reader: RwLock<Option<maxminddb::Reader<Vec<u8>>>>,
    ip2location_path: RwLock<Option<String>>,
}

impl GeoService {
    pub fn new() -> Self {
        Self {
            maxmind_reader: RwLock::new(None),
            ip2location_path: RwLock::new(None),
        }
    }

    /// Load or reload the MaxMind database.
    pub async fn load_maxmind(&self, path: &str) -> Result<()> {
        let reader = maxminddb::Reader::open_readfile(path)?;
        let mut lock = self.maxmind_reader.write().await;
        *lock = Some(reader);
        info!("MaxMind GeoIP database loaded from {}", path);
        Ok(())
    }

    /// Set IP2Location database path.
    pub async fn set_ip2location_path(&self, path: &str) {
        let mut lock = self.ip2location_path.write().await;
        *lock = Some(path.to_string());
        info!("IP2Location database path set to {}", path);
    }

    /// Look up geolocation data for an IP address.
    pub async fn lookup(&self, ip_str: &str) -> GeoData {
        let mut data = GeoData::default();

        // MaxMind lookup for country code
        if let Ok(ip) = ip_str.parse::<IpAddr>() {
            let reader = self.maxmind_reader.read().await;
            if let Some(ref r) = *reader {
                if let Ok(city) = r.lookup::<maxminddb::geoip2::City>(ip) {
                    if let Some(country) = city.country {
                        if let Some(iso) = country.iso_code {
                            data.country_code = iso.to_string();
                        }
                    }
                }
            }
        }

        // IP2Location for datacenter detection
        let ip2loc_path = self.ip2location_path.read().await;
        if let Some(ref _path) = *ip2loc_path {
            data.datacenter = ip2location::is_datacenter_ip(ip_str);
        }

        if data.datacenter {
            data.connection_type = "datacenter".to_string();
        } else {
            data.connection_type = "residential".to_string();
        }

        data
    }
}

#[derive(Debug, Clone)]
pub struct GeoData {
    pub country_code: String,
    pub datacenter: bool,
    pub connection_type: String,
}

impl Default for GeoData {
    fn default() -> Self {
        Self {
            country_code: "XX".to_string(),
            datacenter: false,
            connection_type: "unknown".to_string(),
        }
    }
}
