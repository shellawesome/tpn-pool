pub mod helpers;
pub mod ip2location;
pub mod maxmind;

use crate::db::ip_geo_cache::{read_ip_geo_cache, upsert_ip_geo_cache, IpGeoCacheEntry};
use crate::db::DbPool;
use ::ip2location::DB;
use anyhow::Result;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Known datacenter/hosting provider ASN name patterns (case-insensitive matching).
const DATACENTER_PATTERNS: &[&str] = &[
    "amazon",
    "aws",
    "cloudfront",
    "google",
    "microsoft",
    "azure",
    "digitalocean",
    "linode",
    "vultr",
    "ovh",
    "hetzner",
    "upcloud",
    "scaleway",
    "contabo",
    "ionos",
    "rackspace",
    "softlayer",
    "alibaba",
    "tencent",
    "baidu",
    "cloudflare",
    "fastly",
    "akamai",
    "edgecast",
    "level3",
    "limelight",
    "incapsula",
    "stackpath",
    "maxcdn",
    "cloudsigma",
    "quadranet",
    "psychz",
    "choopa",
    "leaseweb",
    "hostwinds",
    "equinix",
    "colocrossing",
    "hivelocity",
    "godaddy",
    "bluehost",
    "hostgator",
    "dreamhost",
    "hurricane electric",
    "colo",
    "datacenter",
    "serverfarm",
    "hosting",
    "dedicated server",
    "vps",
];

/// Unified geolocation service combining MaxMind and IP2Location.
pub struct GeoService {
    maxmind_reader: RwLock<Option<maxminddb::Reader<Vec<u8>>>>,
    maxmind_asn_reader: RwLock<Option<maxminddb::Reader<Vec<u8>>>>,
    ip2location_db: RwLock<Option<Arc<DB>>>,
}

impl GeoService {
    pub fn new() -> Self {
        Self {
            maxmind_reader: RwLock::new(None),
            maxmind_asn_reader: RwLock::new(None),
            ip2location_db: RwLock::new(None),
        }
    }

    /// Load or reload the MaxMind City database.
    pub async fn load_maxmind(&self, path: &str) -> Result<()> {
        let reader = maxminddb::Reader::open_readfile(path)?;
        let mut lock = self.maxmind_reader.write().await;
        *lock = Some(reader);
        info!("MaxMind GeoIP City database loaded from {}", path);
        Ok(())
    }

    /// Load or reload the MaxMind ASN database.
    pub async fn load_maxmind_asn(&self, path: &str) -> Result<()> {
        let reader = maxminddb::Reader::open_readfile(path)?;
        let mut lock = self.maxmind_asn_reader.write().await;
        *lock = Some(reader);
        info!("MaxMind GeoIP ASN database loaded from {}", path);
        Ok(())
    }

    /// Load or reload the IP2Location ASN database.
    pub async fn load_ip2location(&self, path: &str) -> Result<()> {
        let db = DB::from_file(path)
            .map_err(|e| anyhow::anyhow!("failed to open IP2Location DB {}: {}", path, e))?;
        let mut lock = self.ip2location_db.write().await;
        *lock = Some(Arc::new(db));
        info!("IP2Location database loaded from {}", path);
        Ok(())
    }

    /// Look up geolocation data for an IP address.
    pub async fn lookup(&self, pool: &DbPool, ip_str: &str) -> GeoData {
        let mut data = GeoData::default();

        let ip: IpAddr = match ip_str.parse() {
            Ok(ip) => ip,
            Err(_) => return data,
        };

        // Preferred source 1: local ip.im-backed cache
        if let Ok(Some(cached)) = read_ip_geo_cache(pool, ip_str) {
            if !cached.country_code.trim().is_empty() {
                data.country_code = cached.country_code;
            }
        } else if let Some(entry) = fetch_ip_im_geo(ip_str).await {
            data.country_code = entry.country_code.clone();
            if let Err(e) = upsert_ip_geo_cache(pool, &entry) {
                warn!("Failed to upsert ip.im geo cache for {}: {}", ip_str, e);
            }
        }

        // Preferred source 3: MaxMind City lookup for country code if cache/ip.im did not resolve it.
        if data.country_code == "XX" {
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

        // MaxMind ASN lookup for datacenter detection
        {
            let reader = self.maxmind_asn_reader.read().await;
            if let Some(ref r) = *reader {
                if let Ok(asn) = r.lookup::<maxminddb::geoip2::Asn>(ip) {
                    if let Some(org) = asn.autonomous_system_organization {
                        let org_lower = org.to_lowercase();
                        data.datacenter = DATACENTER_PATTERNS
                            .iter()
                            .any(|pattern| org_lower.contains(pattern));
                    }
                }
            }
        }

        // Fallback: IP2Location ASN lookup when MaxMind does not identify a datacenter.
        if !data.datacenter {
            let db = self.ip2location_db.read().await;
            if let Some(ref db) = *db {
                data.datacenter = ip2location::is_datacenter_ip(db, ip);
            }
        }

        data.connection_type = if data.datacenter {
            "datacenter".to_string()
        } else {
            "residential".to_string()
        };

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

async fn fetch_ip_im_geo(ip: &str) -> Option<IpGeoCacheEntry> {
    let url = format!("https://ip.im/{}", ip);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;

    let body = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, "curl/8.0")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;

    let country_code = parse_ip_im_field(&body, "Country")?;
    let country_code = country_code.trim().to_uppercase();
    if country_code.len() != 2 {
        return None;
    }

    Some(IpGeoCacheEntry {
        ip: ip.to_string(),
        country_code,
        hostname: parse_ip_im_field(&body, "Hostname"),
        city: parse_ip_im_field(&body, "City"),
        region: parse_ip_im_field(&body, "Region"),
        loc: parse_ip_im_field(&body, "Loc"),
        org: parse_ip_im_field(&body, "Org"),
        postal: parse_ip_im_field(&body, "Postal"),
        timezone: parse_ip_im_field(&body, "Timezone"),
        asn: parse_ip_im_field(&body, "ASN"),
        raw_response: Some(body),
        updated_at: chrono::Utc::now().timestamp_millis(),
    })
}

fn parse_ip_im_field(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        let prefix = format!("{}:", key);
        if let Some(value) = trimmed.strip_prefix(&prefix) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}
