mod api;
mod cache;
mod config;
mod dashboard;
mod db;
mod geo;
mod http;
mod locks;
mod networking;
mod partnered_pools;
mod scoring;
mod system;
mod validations;

use cache::TtlCache;
use cache::tpn_cache::TpnCache;
use config::AppConfig;
use db::DbPool;
use geo::GeoService;
use locks::LockRegistry;
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{info, warn};

/// Shared application state passed to all HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: DbPool,
    pub cache: Arc<TtlCache>,
    pub tpn_cache: Arc<TpnCache>,
    pub config: AppConfig,
    pub locks: Arc<LockRegistry>,
    pub geo: Arc<GeoService>,
    pub branch: String,
    pub hash: String,
    pub start_time: chrono::DateTime<chrono::Utc>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load config
    let config = AppConfig::load()?;

    // Initialize logging
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Get git info
    let (branch, hash) = system::shell::get_git_branch_and_hash().await;
    info!(
        "Starting TPN Pool. Version {} ({}/{})",
        env!("CARGO_PKG_VERSION"),
        branch,
        hash
    );
    info!("Using config file {}", config.env_file_path.display());

    // Check system warnings
    system::shell::check_system_warnings().await;

    // Initialize database
    let pool = db::init_pool(&config)?;
    db::init_schema(&pool, &config)?;

    // Initialize caches
    let ttl_cache = Arc::new(TtlCache::new());
    ttl_cache.spawn_eviction_task();

    let tpn_cache = Arc::new(TpnCache::new("./cache/.tpn_cache.json"));
    if let Err(e) = tpn_cache.restore_from_disk().await {
        warn!("Error restoring TPN cache from disk: {}", e);
    }
    tpn_cache.spawn_save_task();

    // Initialize geolocation service
    let geo = Arc::new(GeoService::new());

    info!("Updating geolocation databases...");
    let (maxmind_result, ip2loc_result) = tokio::join!(
        geo::maxmind::update_maxmind(&geo, &pool, config.maxmind_license_key.as_deref()),
        geo::ip2location::update_ip2location(
            &geo,
            &pool,
            config.ip2location_download_token.as_deref()
        ),
    );
    if let Err(e) = maxmind_result {
        warn!("MaxMind update failed: {}", e);
    }
    if let Err(e) = ip2loc_result {
        warn!("IP2Location update failed: {}", e);
    }

    // Clean up network interfaces
    networking::wireguard::clean_up_tpn_interfaces().await;
    networking::wireguard::clean_up_tpn_namespaces().await;

    // Build app state
    let locks = Arc::new(LockRegistry::new());
    let state = AppState {
        db: pool.clone(),
        cache: ttl_cache.clone(),
        tpn_cache: tpn_cache.clone(),
        config: config.clone(),
        locks: locks.clone(),
        geo: geo.clone(),
        branch,
        hash,
        start_time: chrono::Utc::now(),
    };

    // Build router
    let router = http::build_router(&config);

    // Shutdown notifier
    let shutdown_notify = Arc::new(Notify::new());

    // Start background daemons
    let daemon_interval = std::time::Duration::from_secs(config.daemon_interval_seconds);

    // Database cleanup daemon
    {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(daemon_interval);
            loop {
                interval.tick().await;
                if let Err(e) = db::cleanup::database_cleanup(&pool) {
                    warn!("Database cleanup error: {}", e);
                }
            }
        });
    }

    // Geolocation update daemon (daily)
    {
        let geo = geo.clone();
        let pool = pool.clone();
        let maxmind_key = config.maxmind_license_key.clone();
        let ip2loc_token = config.ip2location_download_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            loop {
                interval.tick().await;
                let _ = geo::maxmind::update_maxmind(&geo, &pool, maxmind_key.as_deref()).await;
                let _ = geo::ip2location::update_ip2location(&geo, &pool, ip2loc_token.as_deref()).await;
            }
        });
    }

    // Miner daemons: register with validators + score workers + broadcast workers
    {
        let pool2 = pool.clone();
        let config2 = config.clone();
        let cache2 = ttl_cache.clone();
        let locks2 = locks.clone();
        let geo2 = geo.clone();

        tokio::spawn(async move {
            // Initial validator registration with retry
            loop {
                match api::mining_pool::register_mining_pool_with_validators(&config2, &cache2).await {
                    Ok(successes) if !successes.is_empty() => break,
                    _ => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                }
            }

            // Initial scoring after 30s
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let _ = scoring::score_workers::score_all_known_workers(&pool2, &config2, &locks2, &geo2).await;
            let _ = api::mining_pool::register_mining_pool_workers_with_validators(&pool2, &config2, &cache2).await;

            // Periodic daemons
            let mut interval = tokio::time::interval(daemon_interval);
            loop {
                interval.tick().await;
                let _ = api::mining_pool::register_mining_pool_with_validators(&config2, &cache2).await;
                let _ = scoring::score_workers::score_all_known_workers(&pool2, &config2, &locks2, &geo2).await;
                let _ = api::mining_pool::register_mining_pool_workers_with_validators(&pool2, &config2, &cache2).await;
            }
        });
    }

    // Run initial database cleanup
    let _ = db::cleanup::database_cleanup(&pool);

    // Start HTTP server (blocks until shutdown)
    info!("Starting server on :{}", config.server_port);
    http::server::start_server(router, state, config.server_port, shutdown_notify.clone()).await?;

    // Graceful shutdown: save cache
    info!("Saving TPN cache to disk...");
    if let Err(e) = tpn_cache.save_to_disk().await {
        warn!("Failed to save TPN cache: {}", e);
    }

    info!("TPN Pool shut down successfully");
    Ok(())
}
