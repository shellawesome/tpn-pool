mod api;
mod cache;
mod chain;
mod config;
mod crypto;
mod dashboard;
mod db;
mod geo;
mod http;
mod locks;
mod networking;
mod partnered_pools;
mod scoring;
mod supervisor;
mod system;
mod validations;
mod wallet_files;

use cache::tpn_cache::TpnCache;
use cache::TtlCache;
use config::AppConfig;
use db::DbPool;
use geo::GeoService;
use locks::LockRegistry;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
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

#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    Run,
    Help,
    Config,
    Register,
    Doctor,
}

struct BackendRuntime {
    shutdown_notify: Arc<Notify>,
    server_handle: JoinHandle<anyhow::Result<()>>,
    tpn_cache: Arc<TpnCache>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match parse_cli_command(std::env::args().skip(1))? {
        CliCommand::Help => {
            print!("{}", usage_text());
            return Ok(());
        }
        CliCommand::Config => {
            let env_file_path = config::ensure_env_file()?;
            if let Some(config_dir) = env_file_path.parent() {
                let _ = config::ensure_python_shim_file(config_dir)?;
            }
            let env_contents = config::read_env_file_contents(&env_file_path)?;
            println!("# {}", env_file_path.display());
            print!("{}", env_contents);
            return Ok(());
        }
        CliCommand::Register => {
            let config = AppConfig::load()?;
            wallet_files::materialize_bittensor_wallet_from_env(&config)?;
            chain::run_register_command(&config).await?;
            return Ok(());
        }
        CliCommand::Doctor => {
            let config = AppConfig::load()?;
            wallet_files::materialize_bittensor_wallet_from_env(&config)?;
            run_doctor_command(&config)?;
            return Ok(());
        }
        CliCommand::Run => {}
    }

    // Load config
    let config = AppConfig::load()?;
    wallet_files::materialize_bittensor_wallet_from_env(&config)?;

    // Initialize logging
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Git info (branch hardcoded to main, hash injected at compile time from tpn-subnet main)
    let branch = "main".to_string();
    let hash = env!("TPN_SUBNET_GIT_HASH").to_string();
    info!(
        "Starting TPN Pool. Version {} ({}/{})",
        env!("CARGO_PKG_VERSION"),
        branch,
        hash
    );
    info!("Using config file {}", config.env_file_path.display());
    if supervisor::should_start_python_shim(&config) {
        supervisor::validate_python_shim_config(&config)?;
        supervisor::verify_python_shim_environment(&config).await?;
        info!(
            python = %config.python_bin,
            shim = %config.python_shim_path.display(),
            "Python shim supervision enabled"
        );
    }

    let backend = start_backend(config.clone(), branch, hash).await?;
    let BackendRuntime {
        shutdown_notify,
        server_handle,
        tpn_cache,
    } = backend;

    let result = if supervisor::should_start_python_shim(&config) {
        supervisor::wait_for_backend_ready(config.server_port).await?;
        let shim_task = tokio::spawn(supervisor::supervise_python_shim(config.clone()));
        coordinate_backend_and_shim(server_handle, shutdown_notify.clone(), shim_task).await
    } else {
        wait_for_server(server_handle).await
    };

    info!("Saving TPN cache to disk...");
    if let Err(e) = tpn_cache.save_to_disk().await {
        warn!("Failed to save TPN cache: {}", e);
    }

    info!("TPN Pool shut down successfully");
    result
}

async fn coordinate_backend_and_shim(
    server_handle: JoinHandle<anyhow::Result<()>>,
    shutdown_notify: Arc<Notify>,
    shim_task: JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let mut server_handle = server_handle;
    let mut shim_task = shim_task;

    tokio::select! {
        server_result = &mut server_handle => {
            shim_task.abort();
            wait_for_server_result(server_result)
        }
        shim_result = &mut shim_task => {
            shutdown_notify.notify_waiters();
            let shim_result = shim_result.map_err(anyhow::Error::from)?;
            let server_result = server_handle.await;
            shim_result?;
            wait_for_server_result(server_result)
        }
    }
}

async fn wait_for_server(server_handle: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    wait_for_server_result(server_handle.await)
}

fn wait_for_server_result(
    result: Result<anyhow::Result<()>, tokio::task::JoinError>,
) -> anyhow::Result<()> {
    result.map_err(anyhow::Error::from)?
}

async fn start_backend(
    config: AppConfig,
    branch: String,
    hash: String,
) -> anyhow::Result<BackendRuntime> {
    // Check system warnings
    system::shell::check_system_warnings().await;

    // Initialize database
    let pool = db::init_pool(&config)?;
    db::init_schema(&pool, &config)?;

    // Initialize caches
    let ttl_cache = Arc::new(TtlCache::new());
    ttl_cache.spawn_eviction_task();

    // Restore persisted validator list so initial registration doesn't have to wait
    // for the Python shim's first neuron broadcast.
    match db::validators_cache::load_validators(&pool) {
        Ok(validators) if !validators.is_empty() => {
            info!(
                "Restored {} validators from DB cache",
                validators.len()
            );
            ttl_cache.set_permanent(
                "last_known_validators",
                serde_json::Value::Array(validators),
            );
        }
        Ok(_) => {
            info!("No persisted validators found; will wait for Python shim neuron broadcast");
        }
        Err(e) => warn!("Failed to load validators from DB cache: {}", e),
    }

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
            &config.config_dir,
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
        let config_dir = config.config_dir.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
            loop {
                interval.tick().await;
                let _ = geo::maxmind::update_maxmind(&geo, &pool, maxmind_key.as_deref()).await;
                let _ = geo::ip2location::update_ip2location(
                    &geo,
                    &pool,
                    &config_dir,
                    ip2loc_token.as_deref(),
                )
                .await;
            }
        });
    }

    // Chain registration status sync daemon
    {
        let config = config.clone();
        let tpn_cache = tpn_cache.clone();
        tokio::spawn(async move {
            sync_chain_registration_status(&config, &tpn_cache).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                sync_chain_registration_status(&config, &tpn_cache).await;
            }
        });
    }

    // Metagraph sync daemon: populates last_known_validators directly from chain,
    // replacing the old Python shim broadcast path.
    {
        let config = config.clone();
        let cache = ttl_cache.clone();
        let pool = pool.clone();
        tokio::spawn(async move {
            sync_validators_from_chain(&config, &cache, &pool).await;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                sync_validators_from_chain(&config, &cache, &pool).await;
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
                match api::mining_pool::register_mining_pool_with_validators(&config2, &cache2)
                    .await
                {
                    Ok(successes) if !successes.is_empty() => break,
                    _ => tokio::time::sleep(std::time::Duration::from_secs(5)).await,
                }
            }

            // Initial scoring after 30s
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let _ =
                scoring::score_workers::score_all_known_workers(&pool2, &config2, &locks2, &geo2)
                    .await;
            let _ = api::mining_pool::register_mining_pool_workers_with_validators(
                &pool2, &config2, &cache2,
            )
            .await;

            // Periodic daemons
            let mut interval = tokio::time::interval(daemon_interval);
            loop {
                interval.tick().await;
                let _ =
                    api::mining_pool::register_mining_pool_with_validators(&config2, &cache2).await;
                let _ = scoring::score_workers::score_all_known_workers(
                    &pool2, &config2, &locks2, &geo2,
                )
                .await;
                let _ = api::mining_pool::register_mining_pool_workers_with_validators(
                    &pool2, &config2, &cache2,
                )
                .await;
            }
        });
    }

    // Run initial database cleanup
    let _ = db::cleanup::database_cleanup(&pool);

    // Start HTTP server in the background so the supervisor can manage child processes.
    info!("Starting server on :{}", config.server_port);
    let server_shutdown_notify = shutdown_notify.clone();
    let server_handle = tokio::spawn(async move {
        http::server::start_server(
            router,
            state,
            config.server_port,
            server_shutdown_notify.clone(),
        )
        .await
    });

    Ok(BackendRuntime {
        shutdown_notify,
        server_handle,
        tpn_cache,
    })
}

async fn sync_validators_from_chain(
    config: &AppConfig,
    cache: &Arc<TtlCache>,
    pool: &DbPool,
) {
    match chain::fetch_validators_from_chain(config).await {
        Ok(validators) if !validators.is_empty() => {
            info!(
                "Synced {} validators from chain metagraph",
                validators.len()
            );
            cache.set_permanent(
                "last_known_validators",
                serde_json::Value::Array(validators.clone()),
            );
            if let Err(e) = db::validators_cache::save_validators(pool, &validators) {
                warn!("Failed to persist validators cache to DB: {}", e);
            }
        }
        Ok(_) => {
            warn!("Metagraph sync returned zero validators — leaving cache untouched");
        }
        Err(e) => {
            warn!("Metagraph sync failed: {:#}", e);
        }
    }
}

async fn sync_chain_registration_status(config: &AppConfig, tpn_cache: &Arc<TpnCache>) {
    let now = chrono::Utc::now();
    let value = match chain::fetch_existing_uid(config).await {
        Ok(uid) => serde_json::json!({
            "registered": uid.is_some(),
            "uid": uid,
            "checked_at": now.to_rfc3339(),
            "checked_at_ms": now.timestamp_millis(),
            "error": serde_json::Value::Null,
        }),
        Err(error) => serde_json::json!({
            "registered": false,
            "uid": serde_json::Value::Null,
            "checked_at": now.to_rfc3339(),
            "checked_at_ms": now.timestamp_millis(),
            "error": error.to_string(),
        }),
    };
    tpn_cache.set("dashboard_chain_registration", value).await;
}

fn run_doctor_command(config: &AppConfig) -> anyhow::Result<()> {
    let paths = wallet_files::wallet_paths()?;
    let hotkey_ss58 = wallet_files::derive_hotkey_ss58(config)?;
    let coldkey_ss58 = wallet_files::derive_coldkey_ss58(config)?;

    println!("TPN Pool doctor");
    println!();
    println!("Chain:");
    println!("  network: {}", config.bt_subtensor_network);
    println!(
        "  endpoint: {}",
        config
            .bt_subtensor_chain_endpoint
            .as_deref()
            .unwrap_or("wss://entrypoint-finney.opentensor.ai:443")
    );
    println!("  netuid: {}", config.bt_netuid.unwrap_or(65));
    println!(
        "  external ip: {}",
        config.bt_external_ip.as_deref().unwrap_or("unavailable")
    );
    println!();
    println!("Wallet:");
    println!("  wallet name: {}", wallet_files::DEFAULT_WALLET_NAME);
    println!("  hotkey name: {}", wallet_files::DEFAULT_HOTKEY_NAME);
    println!("  wallet root: {}", paths.wallet_root.display());
    println!("  wallet dir: {}", paths.wallet_dir.display());
    println!("  hotkey ss58: {}", hotkey_ss58);
    println!("  coldkey ss58: {}", coldkey_ss58);
    println!();
    println!("Files:");
    print_file_status("hotkey", &paths.hotkey_path);
    print_file_status("coldkey", &paths.coldkey_path);
    print_file_status("coldkeypub", &paths.coldkeypub_path);
    Ok(())
}

fn print_file_status(label: &str, path: &Path) {
    println!(
        "  {}: {} ({})",
        label,
        path.display(),
        if path.exists() { "present" } else { "missing" }
    );
}

fn parse_cli_command<I>(args: I) -> anyhow::Result<CliCommand>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    if args.is_empty() {
        return Ok(CliCommand::Run);
    }

    match args[0].as_str() {
        "run" if args.len() == 1 => Ok(CliCommand::Run),
        "register" if args.len() == 1 => Ok(CliCommand::Register),
        "doctor" if args.len() == 1 => Ok(CliCommand::Doctor),
        "-h" | "--help" | "help" => Ok(CliCommand::Help),
        "config" if args.len() == 1 => Ok(CliCommand::Config),
        other => Err(anyhow::anyhow!(
            "unknown command '{}'\n\n{}",
            other,
            usage_text()
        )),
    }
}

fn usage_text() -> String {
    let mut usage = String::new();
    let _ = writeln!(&mut usage, "TPN Pool {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(&mut usage);
    let _ = writeln!(&mut usage, "Usage:");
    let _ = writeln!(
        &mut usage,
        "  tpn-pool            Start the miner pool server"
    );
    let _ = writeln!(
        &mut usage,
        "  tpn-pool run        Start the miner pool server"
    );
    let _ = writeln!(
        &mut usage,
        "  tpn-pool register   Print registration details and wait for Enter before submitting"
    );
    let _ = writeln!(
        &mut usage,
        "  tpn-pool doctor     Print derived wallet addresses, paths, and chain config"
    );
    let _ = writeln!(
        &mut usage,
        "  tpn-pool config     Create .env and miner shim if missing, then print .env"
    );
    let _ = writeln!(&mut usage, "  tpn-pool -h");
    let _ = writeln!(&mut usage, "  tpn-pool --help");
    let _ = writeln!(&mut usage, "  tpn-pool help");
    usage
}

#[cfg(test)]
mod tests {
    use super::{parse_cli_command, CliCommand};

    #[test]
    fn parses_help_flags() {
        assert_eq!(
            parse_cli_command(vec!["-h".to_string()]).unwrap(),
            CliCommand::Help
        );
        assert_eq!(
            parse_cli_command(vec!["--help".to_string()]).unwrap(),
            CliCommand::Help
        );
        assert_eq!(
            parse_cli_command(vec!["help".to_string()]).unwrap(),
            CliCommand::Help
        );
    }

    #[test]
    fn parses_config_subcommand() {
        assert_eq!(
            parse_cli_command(vec!["config".to_string()]).unwrap(),
            CliCommand::Config
        );
    }

    #[test]
    fn defaults_to_run_without_args() {
        assert_eq!(
            parse_cli_command(Vec::<String>::new()).unwrap(),
            CliCommand::Run
        );
    }

    #[test]
    fn parses_run_subcommand() {
        assert_eq!(
            parse_cli_command(vec!["run".to_string()]).unwrap(),
            CliCommand::Run
        );
    }

    #[test]
    fn parses_register_subcommand() {
        assert_eq!(
            parse_cli_command(vec!["register".to_string()]).unwrap(),
            CliCommand::Register
        );
    }

    #[test]
    fn parses_doctor_subcommand() {
        assert_eq!(
            parse_cli_command(vec!["doctor".to_string()]).unwrap(),
            CliCommand::Doctor
        );
    }
}
