use crate::db::DbPool;
use anyhow::Result;
use serde_json::{json, Value};

pub fn init_schema(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS validators_cache (
            uid TEXT NOT NULL PRIMARY KEY,
            ip TEXT NOT NULL,
            validator_trust REAL NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

/// Replace the persisted validator list with the given entries.
pub fn save_validators(pool: &DbPool, validators: &[Value]) -> Result<()> {
    let mut conn = pool.get()?;
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM validators_cache", [])?;
    let now = chrono::Utc::now().timestamp_millis();
    for v in validators {
        let uid = v.get("uid").and_then(|x| x.as_str()).unwrap_or("");
        let ip = v.get("ip").and_then(|x| x.as_str()).unwrap_or("");
        let trust = v
            .get("validator_trust")
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        if uid.is_empty() || ip.is_empty() {
            continue;
        }
        tx.execute(
            "INSERT OR REPLACE INTO validators_cache (uid, ip, validator_trust, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![uid, ip, trust, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Load persisted validators as JSON objects matching the in-memory cache format.
pub fn load_validators(pool: &DbPool) -> Result<Vec<Value>> {
    let conn = pool.get()?;
    let mut stmt =
        conn.prepare("SELECT uid, ip, validator_trust FROM validators_cache ORDER BY uid")?;
    let rows = stmt.query_map([], |row| {
        let uid: String = row.get(0)?;
        let ip: String = row.get(1)?;
        let trust: f64 = row.get(2)?;
        Ok(json!({ "uid": uid, "ip": ip, "validator_trust": trust }))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}
