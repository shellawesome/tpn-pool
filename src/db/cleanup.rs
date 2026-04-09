use super::DbPool;
use anyhow::Result;
use chrono::Utc;
use tracing::info;

/// Staleness thresholds in milliseconds.
const STALE_90_MIN_MS: i64 = 90 * 60 * 1000;
const STALE_1_YEAR_MS: i64 = 365 * 24 * 60 * 60 * 1000;

/// Run periodic database cleanup, deleting stale entries.
pub fn database_cleanup(pool: &DbPool) -> Result<()> {
    let conn = pool.get()?;
    let now = Utc::now().timestamp_millis();

    // Workers table: 90 minutes
    let cutoff = now - STALE_90_MIN_MS;
    let count = conn.execute(
        "DELETE FROM workers WHERE updated_at < ?1",
        rusqlite::params![cutoff],
    )?;
    if count > 0 {
        info!("Cleaned up {} stale workers", count);
    }

    // Worker performance: 1 year
    let cutoff = now - STALE_1_YEAR_MS;
    let count = conn.execute(
        "DELETE FROM worker_performance WHERE updated_at < ?1",
        rusqlite::params![cutoff],
    )?;
    if count > 0 {
        info!("Cleaned up {} stale worker_performance entries", count);
    }

    // Challenge solution: 90 minutes
    let cutoff = now - STALE_90_MIN_MS;
    let count = conn.execute(
        "DELETE FROM challenge_solution WHERE updated < ?1",
        rusqlite::params![cutoff],
    )?;
    if count > 0 {
        info!("Cleaned up {} stale challenge_solution entries", count);
    }

    Ok(())
}
