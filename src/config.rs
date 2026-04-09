use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub env_file_path: PathBuf,
    pub server_port: u16,
    pub server_public_protocol: String,
    pub server_public_host: String,
    pub server_public_port: u16,

    // Database
    pub db_path: String,
    pub force_destroy_database: bool,

    // CI
    pub ci_mode: bool,
    pub ci_mock_mining_pool_responses: bool,
    // Geolocation
    pub maxmind_license_key: Option<String>,
    pub ip2location_download_token: Option<String>,

    // Security
    pub lease_token_secret: Option<String>,
    pub admin_api_key: Option<String>,

    // Mining pool
    pub mining_pool_url: Option<String>,
    pub mining_pool_name: Option<String>,
    pub mining_pool_website_url: Option<String>,
    pub mining_pool_rewards: Option<String>,

    // Payment
    pub payment_address_evm: Option<String>,
    pub payment_address_bittensor: Option<String>,

    // Networking
    pub tpn_internal_subnet: String,
    pub tpn_external_subnet: String,

    // Daemon
    pub daemon_interval_seconds: u64,

    // Scoring
    pub force_refresh: bool,

    // Partnered pools
    pub partnered_network_mining_pools: Vec<String>,

    // Logging
    pub log_level: String,

    // Dashboard auth
    pub login_password: String,
    pub jwt_secret: String,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let config_dir = default_config_dir()?;
        fs::create_dir_all(&config_dir)
            .with_context(|| format!("failed to create config dir {}", config_dir.display()))?;

        let env_file_path = config_dir.join(".env");
        ensure_default_env_file(&env_file_path)?;
        load_env_file(&env_file_path)?;

        let ci_mode = env_bool("CI_MODE");
        let default_db_path = config_dir.join("tpn.db");

        Ok(AppConfig {
            config_dir,
            env_file_path,
            server_port: env_u16("SERVER_PUBLIC_PORT", 3000),
            server_public_protocol: env_str("SERVER_PUBLIC_PROTOCOL", "http"),
            server_public_host: env_str("SERVER_PUBLIC_HOST", "localhost"),
            server_public_port: env_u16("SERVER_PUBLIC_PORT", 3000),

            db_path: env::var("DB_PATH")
                .unwrap_or_else(|_| default_db_path.to_string_lossy().into_owned()),
            force_destroy_database: env_bool("FORCE_DESTROY_DATABASE"),

            ci_mode,
            ci_mock_mining_pool_responses: env_bool("CI_MOCK_MINING_POOL_RESPONSES"),
            maxmind_license_key: env::var("MAXMIND_LICENSE_KEY").ok(),
            ip2location_download_token: env::var("IP2LOCATION_DOWNLOAD_TOKEN").ok(),

            lease_token_secret: env::var("LEASE_TOKEN_SECRET").ok(),
            admin_api_key: env::var("ADMIN_API_KEY").ok(),

            mining_pool_url: env::var("MINING_POOL_URL").ok(),
            mining_pool_name: env::var("MINING_POOL_NAME").ok(),
            mining_pool_website_url: env::var("MINING_POOL_WEBSITE_URL").ok(),
            mining_pool_rewards: env::var("MINING_POOL_REWARDS").ok(),

            payment_address_evm: env::var("PAYMENT_ADDRESS_EVM").ok(),
            payment_address_bittensor: env::var("PAYMENT_ADDRESS_BITTENSOR").ok(),

            tpn_internal_subnet: env_str("TPN_INTERNAL_SUBNET", "10.13.13.0/24"),
            tpn_external_subnet: env_str("TPN_EXTERNAL_SUBNET", "10.14.14.0/24"),

            daemon_interval_seconds: env_u64(
                "DAEMON_INTERVAL_SECONDS",
                if ci_mode { 60 } else { 300 },
            ),

            force_refresh: env_bool("FORCE_REFRESH"),

            partnered_network_mining_pools: env::var("PARTNERED_NETWORK_MINING_POOLS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),

            log_level: env_str("LOG_LEVEL", "info"),

            login_password: env::var("LOGIN_PASSWORD").unwrap_or_default(),
            jwt_secret: env::var("JWT_SECRET").unwrap_or_else(|_| "default-secret-change-me".to_string()),
        })
    }

    pub fn base_url(&self) -> String {
        format!(
            "{}://{}:{}",
            self.server_public_protocol, self.server_public_host, self.server_public_port
        )
    }
}

fn default_config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join("tpn-pool"))
        .context("could not determine config directory")
}

fn ensure_default_env_file(env_file_path: &Path) -> Result<()> {
    if env_file_path.exists() {
        return Ok(());
    }

    fs::write(env_file_path, default_env_contents())
        .with_context(|| format!("failed to write default env file {}", env_file_path.display()))?;
    Ok(())
}

fn load_env_file(env_file_path: &Path) -> Result<()> {
    let contents = fs::read_to_string(env_file_path)
        .with_context(|| format!("failed to read env file {}", env_file_path.display()))?;

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() || env::var_os(key).is_some() {
            continue;
        }

        let value = strip_optional_quotes(value.trim());
        // SAFETY: startup-time configuration loading happens before background tasks start.
        unsafe {
            env::set_var(key, value);
        }
    }

    Ok(())
}

fn strip_optional_quotes(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn default_env_contents() -> String {
    [
        "# tpn-pool default configuration",
        "SERVER_PUBLIC_PORT=3000",
        "SERVER_PUBLIC_PROTOCOL=http",
        "SERVER_PUBLIC_HOST=localhost",
        "LOG_LEVEL=info",
        "DAEMON_INTERVAL_SECONDS=300",
        "TPN_INTERNAL_SUBNET=10.13.13.0/24",
        "TPN_EXTERNAL_SUBNET=10.14.14.0/24",
        "FORCE_DESTROY_DATABASE=false",
        "CI_MODE=false",
        "CI_MOCK_MINING_POOL_RESPONSES=false",
        "FORCE_REFRESH=false",
        "LOGIN_PASSWORD=",
        "JWT_SECRET=default-secret-change-me",
        "MINING_POOL_URL=",
        "MINING_POOL_NAME=",
        "MINING_POOL_WEBSITE_URL=",
        "MINING_POOL_REWARDS=",
        "PAYMENT_ADDRESS_EVM=",
        "PAYMENT_ADDRESS_BITTENSOR=",
        "ADMIN_API_KEY=",
        "LEASE_TOKEN_SECRET=",
        "MAXMIND_LICENSE_KEY=",
        "IP2LOCATION_DOWNLOAD_TOKEN=",
        "PARTNERED_NETWORK_MINING_POOLS=",
        "",
    ]
    .join("\n")
}

fn env_str(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str) -> bool {
    env::var(key).map(|v| v == "true").unwrap_or(false)
}

fn env_u16(key: &str, default: u16) -> u16 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
