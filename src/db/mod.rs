pub mod challenge_response;
pub mod cleanup;
pub mod timestamps;
pub mod workers;

use crate::config::AppConfig;
use anyhow::Result;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use tracing::info;

pub type DbPool = Pool<SqliteConnectionManager>;

/// Initialize the SQLite database pool with WAL mode.
pub fn init_pool(config: &AppConfig) -> Result<DbPool> {
    let manager = SqliteConnectionManager::file(&config.db_path);
    let pool = Pool::builder().max_size(10).build(manager)?;

    // Enable WAL mode and set busy timeout
    let conn = pool.get()?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;")?;

    info!("SQLite database pool initialized at {}", config.db_path);
    Ok(pool)
}

/// Create all tables and indexes for mining pool mode.
pub fn init_schema(pool: &DbPool, config: &AppConfig) -> Result<()> {
    let conn = pool.get()?;

    // In CI or force destroy mode, drop old tables
    if config.ci_mode || config.force_destroy_database {
        info!("Dropping old tables (CI/force destroy mode)");
        conn.execute_batch(
            "DROP TABLE IF EXISTS workers;
             DROP TABLE IF EXISTS worker_performance;
             DROP TABLE IF EXISTS timestamps;
             DROP TABLE IF EXISTS challenge_solution;",
        )?;
    }

    // WORKERS table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workers (
            ip TEXT NOT NULL,
            public_url TEXT,
            payment_address_evm TEXT,
            payment_address_bittensor TEXT,
            public_port TEXT NOT NULL,
            country_code TEXT NOT NULL,
            mining_pool_url TEXT NOT NULL,
            mining_pool_uid TEXT NOT NULL,
            status TEXT NOT NULL,
            connection_type TEXT NOT NULL DEFAULT 'unknown',
            updated_at INTEGER NOT NULL,
            PRIMARY KEY (mining_pool_uid, mining_pool_url, ip)
        );
        CREATE INDEX IF NOT EXISTS idx_workers_status_country
            ON workers (status, country_code, connection_type);
        CREATE INDEX IF NOT EXISTS idx_workers_updated_at
            ON workers (updated_at);",
    )?;

    // Partial unique index: one 'up' per IP
    if let Err(e) = conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_workers_single_up_ip
            ON workers (ip) WHERE status = 'up';",
    ) {
        tracing::warn!("Could not create one-up-per-ip index: {}, attempting dedup", e);
        conn.execute_batch(
            "DELETE FROM workers WHERE rowid NOT IN (
                SELECT MIN(rowid) FROM workers WHERE status = 'up' GROUP BY ip
            ) AND status = 'up';
            CREATE UNIQUE INDEX IF NOT EXISTS idx_workers_single_up_ip
                ON workers (ip) WHERE status = 'up';",
        )?;
        info!("Created one-up-per-ip index after dedup");
    }
    info!("Workers table initialized");

    // WORKER_PERFORMANCE table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS worker_performance (
            ip TEXT NOT NULL,
            status TEXT NOT NULL,
            public_url TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_worker_performance_updated_at
            ON worker_performance (updated_at);",
    )?;
    info!("Worker performance table initialized");

    // CHALLENGE_SOLUTION table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS challenge_solution (
            challenge TEXT NOT NULL PRIMARY KEY,
            solution TEXT NOT NULL,
            updated INTEGER NOT NULL
        );",
    )?;
    info!("Challenge solution table initialized");

    // TIMESTAMPS table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS timestamps (
            label TEXT PRIMARY KEY,
            timestamp INTEGER NOT NULL,
            updated INTEGER NOT NULL
        );",
    )?;
    info!("Timestamps table initialized");

    info!("Database schema initialization complete");
    Ok(())
}
