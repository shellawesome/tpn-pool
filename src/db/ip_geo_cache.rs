use super::DbPool;
use anyhow::Result;
use chrono::Utc;

#[derive(Debug, Clone)]
pub struct IpGeoCacheEntry {
    pub ip: String,
    pub country_code: String,
    pub hostname: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub loc: Option<String>,
    pub org: Option<String>,
    pub postal: Option<String>,
    pub timezone: Option<String>,
    pub asn: Option<String>,
    pub raw_response: Option<String>,
    #[allow(dead_code)]
    pub updated_at: i64,
}

pub fn read_ip_geo_cache(pool: &DbPool, ip: &str) -> Result<Option<IpGeoCacheEntry>> {
    let conn = pool.get()?;
    let mut stmt = conn.prepare(
        "SELECT ip, country_code, hostname, city, region, loc, org, postal, timezone, asn, raw_response, updated_at
         FROM ip_geo_cache WHERE ip = ?1",
    )?;

    let entry = stmt
        .query_row(rusqlite::params![ip], |row| {
            Ok(IpGeoCacheEntry {
                ip: row.get(0)?,
                country_code: row.get(1)?,
                hostname: row.get(2)?,
                city: row.get(3)?,
                region: row.get(4)?,
                loc: row.get(5)?,
                org: row.get(6)?,
                postal: row.get(7)?,
                timezone: row.get(8)?,
                asn: row.get(9)?,
                raw_response: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .ok();

    Ok(entry)
}

pub fn upsert_ip_geo_cache(pool: &DbPool, entry: &IpGeoCacheEntry) -> Result<()> {
    let conn = pool.get()?;
    let now = Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO ip_geo_cache (
            ip, country_code, hostname, city, region, loc, org, postal, timezone, asn, raw_response, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (ip) DO UPDATE SET
            country_code = excluded.country_code,
            hostname = excluded.hostname,
            city = excluded.city,
            region = excluded.region,
            loc = excluded.loc,
            org = excluded.org,
            postal = excluded.postal,
            timezone = excluded.timezone,
            asn = excluded.asn,
            raw_response = excluded.raw_response,
            updated_at = excluded.updated_at",
        rusqlite::params![
            entry.ip,
            entry.country_code,
            entry.hostname,
            entry.city,
            entry.region,
            entry.loc,
            entry.org,
            entry.postal,
            entry.timezone,
            entry.asn,
            entry.raw_response,
            now,
        ],
    )?;
    Ok(())
}
