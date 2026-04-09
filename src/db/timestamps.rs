use super::DbPool;
use anyhow::Result;
use chrono::Utc;

/// Get a named timestamp, returns 0 if not found.
pub fn get_timestamp(pool: &DbPool, label: &str) -> Result<i64> {
    let conn = pool.get()?;
    let ts = conn
        .query_row(
            "SELECT timestamp FROM timestamps WHERE label = ?1",
            rusqlite::params![label],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0);
    Ok(ts)
}

/// Set a named timestamp (upsert).
pub fn set_timestamp(pool: &DbPool, label: &str, timestamp: i64) -> Result<()> {
    let conn = pool.get()?;
    let now = Utc::now().timestamp_millis();
    conn.execute(
        "INSERT INTO timestamps (label, timestamp, updated)
         VALUES (?1, ?2, ?3)
         ON CONFLICT (label) DO UPDATE SET
            timestamp = excluded.timestamp,
            updated = excluded.updated",
        rusqlite::params![label, timestamp, now],
    )?;
    Ok(())
}
