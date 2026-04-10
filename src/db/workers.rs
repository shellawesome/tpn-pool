use super::DbPool;
use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worker {
    pub ip: String,
    #[serde(default)]
    pub public_url: Option<String>,
    #[serde(default)]
    pub payment_address_evm: Option<String>,
    #[serde(default)]
    pub payment_address_bittensor: Option<String>,
    pub public_port: String,
    #[serde(default = "default_country")]
    pub country_code: String,
    #[serde(default)]
    pub mining_pool_url: String,
    #[serde(default)]
    pub mining_pool_uid: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_connection_type")]
    pub connection_type: String,
    #[serde(default)]
    pub updated_at: i64,

    // Runtime fields (not stored in DB directly)
    #[serde(default)]
    pub wireguard_config: Option<String>,
    #[serde(default)]
    pub socks5_config: Option<String>,
    #[serde(default)]
    pub text_config: Option<String>,
    #[serde(default)]
    pub success: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub test_duration_s: Option<f64>,
    #[serde(default)]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub observed_egress_ip: Option<String>,
    #[serde(default)]
    pub claimed_worker_ip: Option<String>,
    #[serde(default)]
    pub datacenter: Option<bool>,
}

fn default_country() -> String {
    "XX".to_string()
}
fn default_status() -> String {
    "unknown".to_string()
}
fn default_connection_type() -> String {
    "unknown".to_string()
}

#[derive(Debug, Default)]
pub struct GetWorkersParams {
    pub mining_pool_uid: Option<String>,
    pub status: Option<String>,
    pub worker_ip: Option<String>,
    pub country_code: Option<String>,
    pub connection_type: Option<String>,
    pub whitelist: Option<Vec<String>>,
    pub blacklist: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub randomize: bool,
}

/// Write (upsert) workers into the database.
/// Uses BEGIN IMMEDIATE to serialize writes (replacing pg_advisory_xact_lock).
pub fn write_workers(
    pool: &DbPool,
    workers: &[Worker],
    mining_pool_uid: &str,
    _mining_pool_ip: &str,
) -> Result<()> {
    if workers.is_empty() {
        return Ok(());
    }

    let conn = pool.get()?;
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let now = Utc::now().timestamp_millis();

    // Collect IPs that should be 'up'
    let up_ips: Vec<&str> = workers
        .iter()
        .filter(|w| w.status == "up")
        .map(|w| w.ip.as_str())
        .collect();

    // Demote existing 'up' workers for these IPs (one-up-per-ip invariant)
    for ip in &up_ips {
        conn.execute(
            "UPDATE workers SET status = 'unknown' WHERE status = 'up' AND ip = ?1",
            rusqlite::params![ip],
        )?;
    }

    // Upsert each worker
    for worker in workers {
        let pool_uid = if worker.mining_pool_uid.is_empty() {
            mining_pool_uid
        } else {
            &worker.mining_pool_uid
        };

        conn.execute(
            "INSERT INTO workers (ip, public_url, payment_address_evm, payment_address_bittensor,
                public_port, country_code, mining_pool_url, mining_pool_uid, status,
                connection_type, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT (mining_pool_uid, mining_pool_url, ip) DO UPDATE SET
                public_url = excluded.public_url,
                payment_address_evm = excluded.payment_address_evm,
                payment_address_bittensor = excluded.payment_address_bittensor,
                public_port = excluded.public_port,
                country_code = excluded.country_code,
                status = excluded.status,
                connection_type = excluded.connection_type,
                updated_at = excluded.updated_at",
            rusqlite::params![
                worker.ip,
                worker.public_url,
                worker.payment_address_evm,
                worker.payment_address_bittensor,
                worker.public_port,
                worker.country_code,
                worker.mining_pool_url,
                pool_uid,
                worker.status,
                worker.connection_type,
                now,
            ],
        )?;
    }

    conn.execute_batch("COMMIT")?;
    info!(
        "Wrote {} workers for pool {}",
        workers.len(),
        mining_pool_uid
    );
    Ok(())
}

/// Get workers with flexible filtering.
pub fn get_workers(pool: &DbPool, params: &GetWorkersParams) -> Result<Vec<Worker>> {
    let conn = pool.get()?;

    let mut sql = String::from("SELECT ip, public_url, payment_address_evm, payment_address_bittensor, public_port, country_code, mining_pool_url, mining_pool_uid, status, connection_type, updated_at FROM workers WHERE 1=1");
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(ref uid) = params.mining_pool_uid {
        sql.push_str(&format!(" AND mining_pool_uid = ?{}", param_idx));
        bind_values.push(Box::new(uid.clone()));
        param_idx += 1;
    }

    if let Some(ref status) = params.status {
        sql.push_str(&format!(" AND status = ?{}", param_idx));
        bind_values.push(Box::new(status.clone()));
        param_idx += 1;
    }

    if let Some(ref worker_ip) = params.worker_ip {
        sql.push_str(&format!(" AND ip = ?{}", param_idx));
        bind_values.push(Box::new(worker_ip.clone()));
        param_idx += 1;
    }

    if let Some(ref cc) = params.country_code {
        if cc.to_uppercase() != "ANY" {
            sql.push_str(&format!(" AND country_code = ?{}", param_idx));
            bind_values.push(Box::new(cc.clone()));
            param_idx += 1;
        }
    }

    if let Some(ref ct) = params.connection_type {
        if ct != "any" {
            sql.push_str(&format!(" AND connection_type = ?{}", param_idx));
            bind_values.push(Box::new(ct.clone()));
            param_idx += 1;
        }
    }

    if let Some(ref whitelist) = params.whitelist {
        if !whitelist.is_empty() {
            let placeholders: Vec<String> = whitelist
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_idx + i))
                .collect();
            sql.push_str(&format!(" AND ip IN ({})", placeholders.join(",")));
            for ip in whitelist {
                bind_values.push(Box::new(ip.clone()));
            }
            param_idx += whitelist.len();
        }
    }

    if let Some(ref blacklist) = params.blacklist {
        if !blacklist.is_empty() {
            let placeholders: Vec<String> = blacklist
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_idx + i))
                .collect();
            sql.push_str(&format!(" AND ip NOT IN ({})", placeholders.join(",")));
            for ip in blacklist {
                bind_values.push(Box::new(ip.clone()));
            }
            param_idx += blacklist.len();
        }
    }

    if params.randomize {
        sql.push_str(" ORDER BY RANDOM()");
    }

    if let Some(limit) = params.limit {
        sql.push_str(&format!(" LIMIT ?{}", param_idx));
        bind_values.push(Box::new(limit));
        // param_idx += 1;
    }

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok(Worker {
            ip: row.get(0)?,
            public_url: row.get(1)?,
            payment_address_evm: row.get(2)?,
            payment_address_bittensor: row.get(3)?,
            public_port: row.get(4)?,
            country_code: row.get(5)?,
            mining_pool_url: row.get(6)?,
            mining_pool_uid: row.get(7)?,
            status: row.get(8)?,
            connection_type: row.get(9)?,
            updated_at: row.get(10)?,
            wireguard_config: None,
            socks5_config: None,
            text_config: None,
            success: None,
            error: None,
            test_duration_s: None,
            failure_code: None,
            observed_egress_ip: None,
            claimed_worker_ip: None,
            datacenter: None,
        })
    })?;

    let workers: Vec<Worker> = rows.filter_map(|r| r.ok()).collect();
    Ok(workers)
}

/// Find workers with clashing IPs across pools.
pub fn find_clashing_workers(
    pool: &DbPool,
    workers: &[Worker],
    mining_pool_uid: &str,
) -> Result<Vec<Worker>> {
    if workers.is_empty() {
        return Ok(vec![]);
    }

    let conn = pool.get()?;
    let ips: Vec<&str> = workers.iter().map(|w| w.ip.as_str()).collect();
    let placeholders: Vec<String> = (1..=ips.len()).map(|i| format!("?{}", i)).collect();

    let sql = format!(
        "SELECT ip, public_url, payment_address_evm, payment_address_bittensor, public_port,
                country_code, mining_pool_url, mining_pool_uid, status, connection_type, updated_at
         FROM workers
         WHERE status = 'up' AND ip IN ({}) AND mining_pool_uid != ?{}",
        placeholders.join(","),
        ips.len() + 1
    );

    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    for ip in &ips {
        bind_values.push(Box::new(ip.to_string()));
    }
    bind_values.push(Box::new(mining_pool_uid.to_string()));

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok(Worker {
            ip: row.get(0)?,
            public_url: row.get(1)?,
            payment_address_evm: row.get(2)?,
            payment_address_bittensor: row.get(3)?,
            public_port: row.get(4)?,
            country_code: row.get(5)?,
            mining_pool_url: row.get(6)?,
            mining_pool_uid: row.get(7)?,
            status: row.get(8)?,
            connection_type: row.get(9)?,
            updated_at: row.get(10)?,
            ..Default::default()
        })
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Write worker performance log (append-only).
pub fn write_worker_performance(pool: &DbPool, workers: &[Worker]) -> Result<()> {
    if workers.is_empty() {
        return Ok(());
    }
    let conn = pool.get()?;
    let now = Utc::now().timestamp_millis();
    let mut stmt = conn.prepare(
        "INSERT INTO worker_performance (ip, status, public_url, updated_at) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for worker in workers {
        stmt.execute(rusqlite::params![
            worker.ip,
            worker.status,
            worker.public_url.as_deref().unwrap_or(""),
            now,
        ])?;
    }
    Ok(())
}

/// Get worker performance history for a time range.
pub fn get_worker_performance(
    pool: &DbPool,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<Vec<(String, String, String, i64)>> {
    let conn = pool.get()?;
    let mut sql =
        String::from("SELECT ip, status, public_url, updated_at FROM worker_performance WHERE 1=1");
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(from_ts) = from {
        sql.push_str(&format!(" AND updated_at >= ?{}", param_idx));
        bind_values.push(Box::new(from_ts));
        param_idx += 1;
    }
    if let Some(to_ts) = to {
        sql.push_str(&format!(" AND updated_at <= ?{}", param_idx));
        bind_values.push(Box::new(to_ts));
        // param_idx += 1;
    }
    sql.push_str(" ORDER BY updated_at ASC");

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Get unique country codes for a mining pool (or all pools).
pub fn get_worker_countries_for_pool(
    pool: &DbPool,
    mining_pool_uid: Option<&str>,
    connection_type: Option<&str>,
) -> Result<Vec<String>> {
    let conn = pool.get()?;
    let mut sql = String::from("SELECT DISTINCT country_code FROM workers WHERE status = 'up'");
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut param_idx = 1;

    if let Some(uid) = mining_pool_uid {
        sql.push_str(&format!(" AND mining_pool_uid = ?{}", param_idx));
        bind_values.push(Box::new(uid.to_string()));
        param_idx += 1;
    }

    if let Some(ct) = connection_type {
        if ct != "any" {
            sql.push_str(&format!(" AND connection_type = ?{}", param_idx));
            bind_values.push(Box::new(ct.to_string()));
            // param_idx += 1;
        }
    }

    let mut stmt = conn.prepare(&sql)?;
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(|b| b.as_ref()).collect();
    let rows = stmt.query_map(refs.as_slice(), |row| row.get::<_, String>(0))?;

    Ok(rows.filter_map(|r| r.ok()).collect())
}

impl Default for Worker {
    fn default() -> Self {
        Self {
            ip: String::new(),
            public_url: None,
            payment_address_evm: None,
            payment_address_bittensor: None,
            public_port: "3000".to_string(),
            country_code: "XX".to_string(),
            mining_pool_url: String::new(),
            mining_pool_uid: String::new(),
            status: "unknown".to_string(),
            connection_type: "unknown".to_string(),
            updated_at: 0,
            wireguard_config: None,
            socks5_config: None,
            text_config: None,
            success: None,
            error: None,
            test_duration_s: None,
            failure_code: None,
            observed_egress_ip: None,
            claimed_worker_ip: None,
            datacenter: None,
        }
    }
}
