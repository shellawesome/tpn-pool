use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub config_dir: PathBuf,
    pub env_file_path: PathBuf,
    pub python_shim_path: PathBuf,
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
    pub broadcast_message: Option<String>,
    pub contact_method: Option<String>,

    // Reported version/branch/hash (override compile-time defaults)
    pub reported_version: Option<String>,
    pub reported_branch: Option<String>,
    pub reported_hash: Option<String>,

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

    // Python shim supervisor
    pub python_shim_enabled: bool,
    pub python_bin: String,
    pub sybil_python_root: PathBuf,
    pub bt_netuid: Option<u16>,
    pub bt_subtensor_network: String,
    pub bt_subtensor_chain_endpoint: Option<String>,
    pub bt_hotkey_mnemonic: Option<String>,
    pub bt_hotkey_seed_hex: Option<String>,
    pub bt_coldkey_mnemonic: Option<String>,
    pub bt_coldkey_seed_hex: Option<String>,
    pub bt_axon_port: u16,
    pub bt_external_ip: Option<String>,
    pub bt_force_validator_permit: bool,
    pub bt_allow_non_registered: bool,
    pub python_shim_restart_delay_seconds: u64,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let env_file_path = ensure_env_file()?;
        let config_dir = env_file_path
            .parent()
            .map(Path::to_path_buf)
            .context("could not determine config directory from env file path")?;
        load_env_file(&env_file_path)?;
        let python_shim_path = ensure_python_shim_file(&config_dir)?;
        let sybil_python_root = ensure_sybil_package(&config_dir)?;

        let ci_mode = env_bool("CI_MODE");
        let default_db_path = config_dir.join("tpn.db");

        Ok(AppConfig {
            config_dir,
            env_file_path,
            python_shim_path,
            server_port: env_u16("SERVER_PUBLIC_PORT", 3000),
            server_public_protocol: env_str("SERVER_PUBLIC_PROTOCOL", "http"),
            server_public_host: env_str("SERVER_PUBLIC_HOST", "localhost"),
            server_public_port: env_u16("SERVER_PUBLIC_PORT", 3000),

            db_path: env::var("DB_PATH")
                .unwrap_or_else(|_| default_db_path.to_string_lossy().into_owned()),
            force_destroy_database: env_bool("FORCE_DESTROY_DATABASE"),

            ci_mode,
            ci_mock_mining_pool_responses: env_bool("CI_MOCK_MINING_POOL_RESPONSES"),
            maxmind_license_key: env_opt("MAXMIND_LICENSE_KEY"),
            ip2location_download_token: env_opt("IP2LOCATION_DOWNLOAD_TOKEN"),

            lease_token_secret: env_opt("LEASE_TOKEN_SECRET"),
            admin_api_key: env_opt("ADMIN_API_KEY"),

            mining_pool_url: env_opt("MINING_POOL_URL"),
            mining_pool_name: env_opt("MINING_POOL_NAME"),
            mining_pool_website_url: env_opt("MINING_POOL_WEBSITE_URL"),
            mining_pool_rewards: env_opt("MINING_POOL_REWARDS"),
            broadcast_message: env_opt("BROADCAST_MESSAGE"),
            contact_method: env_opt("CONTACT_METHOD"),

            reported_version: env_opt("VERSION"),
            reported_branch: env_opt("BRANCH"),
            reported_hash: env_opt("HASH"),

            payment_address_evm: env_opt("PAYMENT_ADDRESS_EVM"),
            payment_address_bittensor: env_opt("PAYMENT_ADDRESS_BITTENSOR"),

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
            jwt_secret: env::var("JWT_SECRET")
                .unwrap_or_else(|_| "default-secret-change-me".to_string()),

            python_shim_enabled: env_bool("PYTHON_SHIM_ENABLED"),
            python_bin: env_str("PYTHON_BIN", "python3"),
            sybil_python_root,
            bt_netuid: env::var("BT_NETUID")
                .ok()
                .and_then(|v| v.parse().ok())
                .or(Some(65)),
            bt_subtensor_network: env_str("BT_SUBTENSOR_NETWORK", "finney"),
            bt_subtensor_chain_endpoint: env_opt("BT_SUBTENSOR_CHAIN_ENDPOINT").or(Some(
                "wss://entrypoint-finney.opentensor.ai:443".to_string(),
            )),
            bt_hotkey_mnemonic: env_opt("BT_HOTKEY_MNEMONIC"),
            bt_hotkey_seed_hex: env_opt("BT_HOTKEY_SEED_HEX"),
            bt_coldkey_mnemonic: env_opt("BT_COLDKEY_MNEMONIC"),
            bt_coldkey_seed_hex: env_opt("BT_COLDKEY_SEED_HEX"),
            bt_axon_port: env_u16("BT_AXON_PORT", 8091),
            bt_external_ip: env_opt("BT_EXTERNAL_IP").or_else(detect_external_ip_via_curl),
            bt_force_validator_permit: env_bool_default("BT_FORCE_VALIDATOR_PERMIT", true),
            bt_allow_non_registered: env_bool("BT_ALLOW_NON_REGISTERED"),
            python_shim_restart_delay_seconds: env_u64("PYTHON_SHIM_RESTART_DELAY_SECONDS", 5),
        })
    }

    pub fn base_url(&self) -> String {
        format!(
            "{}://{}:{}",
            self.server_public_protocol, self.server_public_host, self.server_public_port
        )
    }
}

pub fn ensure_env_file() -> Result<PathBuf> {
    let config_dir = default_config_dir()?;
    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create config dir {}", config_dir.display()))?;

    let env_file_path = config_dir.join(".env");
    ensure_default_env_file(&env_file_path)?;
    Ok(env_file_path)
}

pub fn read_env_file_contents(env_file_path: &Path) -> Result<String> {
    fs::read_to_string(env_file_path)
        .with_context(|| format!("failed to read env file {}", env_file_path.display()))
}

pub fn ensure_python_shim_file(config_dir: &Path) -> Result<PathBuf> {
    let shim_path = config_dir.join("miner_shim.py");
    fs::write(&shim_path, default_python_shim_contents()).with_context(|| {
        format!(
            "failed to write default python shim {}",
            shim_path.display()
        )
    })?;
    Ok(shim_path)
}

pub fn ensure_sybil_package(config_dir: &Path) -> Result<PathBuf> {
    let sybil_dir = config_dir.join("sybil");
    fs::create_dir_all(&sybil_dir).with_context(|| format!("creating {}", sybil_dir.display()))?;

    let init_path = sybil_dir.join("__init__.py");
    fs::write(&init_path, sybil_init_contents())
        .with_context(|| format!("writing {}", init_path.display()))?;

    let protocol_path = sybil_dir.join("protocol.py");
    fs::write(&protocol_path, sybil_protocol_contents())
        .with_context(|| format!("writing {}", protocol_path.display()))?;

    Ok(config_dir.to_path_buf())
}

fn sybil_init_contents() -> &'static str {
    "from . import protocol\n"
}

fn sybil_protocol_contents() -> &'static str {
    r#"import typing
import bittensor as bt


class Dummy(bt.Synapse):
    dummy_input: int
    dummy_output: typing.Optional[int] = None

    def deserialize(self) -> int:
        return self.dummy_output


class Challenge(bt.Synapse):
    challenge: str
    challenge_url: str
    challenge_response: typing.Optional[str] = None

    def deserialize(self) -> str:
        return self.challenge_response
"#
}

fn default_config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join("tpn-pool"))
        .context("could not determine config directory")
}

fn ensure_default_env_file(env_file_path: &Path) -> Result<()> {
    if !env_file_path.exists() {
        fs::write(env_file_path, default_env_contents()).with_context(|| {
            format!(
                "failed to write default env file {}",
                env_file_path.display()
            )
        })?;
        return Ok(());
    }

    sync_missing_env_defaults(env_file_path)?;
    Ok(())
}

fn sync_missing_env_defaults(env_file_path: &Path) -> Result<()> {
    let existing = fs::read_to_string(env_file_path)
        .with_context(|| format!("failed to read env file {}", env_file_path.display()))?;

    let mut normalized_lines = Vec::new();
    let mut seen_keys = std::collections::HashSet::new();
    let obsolete_keys = [
        "BT_WALLET_NAME",
        "BT_WALLET_HOTKEY",
        "BT_WALLET_PATH",
        "TPN_SUBNET_PYTHON_ROOT",
    ];

    for raw_line in existing.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            normalized_lines.push(raw_line.to_string());
            continue;
        }

        let Some((key, value)) = raw_line.split_once('=') else {
            normalized_lines.push(raw_line.to_string());
            continue;
        };

        let key = key.trim();
        if obsolete_keys.contains(&key) {
            continue;
        }

        let updated_value = migrate_env_value(key, value.trim());
        normalized_lines.push(format!("{key}={updated_value}"));
        seen_keys.insert(key.to_string());
    }

    let mut missing_lines = Vec::new();
    for line in default_env_contents().lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let Some((key, _)) = trimmed.split_once('=') else {
            continue;
        };
        if !seen_keys.contains(key.trim()) {
            missing_lines.push(trimmed.to_string());
        }
    }

    if missing_lines.is_empty() && normalized_lines.join("\n") == existing {
        return Ok(());
    }

    let mut updated = normalized_lines.join("\n");
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    if !missing_lines.is_empty() {
        updated.push_str("\n# Added by newer tpn-pool version\n");
        updated.push_str(&missing_lines.join("\n"));
        updated.push('\n');
    }

    fs::write(env_file_path, updated)
        .with_context(|| format!("failed to update env file {}", env_file_path.display()))?;
    Ok(())
}

fn migrate_env_value(key: &str, current_value: &str) -> String {
    if !current_value.is_empty() {
        return current_value.to_string();
    }

    match key {
        "BT_NETUID" => "65".to_string(),
        "BT_SUBTENSOR_NETWORK" => "finney".to_string(),
        "BT_SUBTENSOR_CHAIN_ENDPOINT" => "wss://entrypoint-finney.opentensor.ai:443".to_string(),
        _ => current_value.to_string(),
    }
}

fn detect_external_ip_via_curl() -> Option<String> {
    let output = std::process::Command::new("curl")
        .arg("-s")
        .arg("3.0.3.0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    json.get("ip")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
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
        "BROADCAST_MESSAGE=",
        "CONTACT_METHOD=",
        "VERSION=",
        "BRANCH=",
        "HASH=",
        "PAYMENT_ADDRESS_EVM=",
        "PAYMENT_ADDRESS_BITTENSOR=",
        "ADMIN_API_KEY=",
        "LEASE_TOKEN_SECRET=",
        "MAXMIND_LICENSE_KEY=",
        "IP2LOCATION_DOWNLOAD_TOKEN=",
        "PARTNERED_NETWORK_MINING_POOLS=",
        "",
        "# Optional Python axon shim supervisor",
        "PYTHON_SHIM_ENABLED=false",
        "PYTHON_BIN=python3",
        "BT_NETUID=65",
        "BT_SUBTENSOR_NETWORK=finney",
        "BT_SUBTENSOR_CHAIN_ENDPOINT=wss://entrypoint-finney.opentensor.ai:443",
        "BT_HOTKEY_MNEMONIC=",
        "BT_HOTKEY_SEED_HEX=",
        "BT_COLDKEY_MNEMONIC=",
        "BT_COLDKEY_SEED_HEX=",
        "BT_AXON_PORT=8091",
        "BT_EXTERNAL_IP=",
        "BT_FORCE_VALIDATOR_PERMIT=true",
        "BT_ALLOW_NON_REGISTERED=false",
        "PYTHON_SHIM_RESTART_DELAY_SECONDS=5",
        "",
    ]
    .join("\n")
}

fn default_python_shim_contents() -> &'static str {
    r#"#!/usr/bin/env python3
import argparse
import asyncio
import json
import os
import sys
import time
import typing
import urllib.request

import bittensor as bt

TPN_SUBNET_ROOT = os.environ.get("TPN_SUBNET_PYTHON_ROOT")
if TPN_SUBNET_ROOT:
    sys.path.insert(0, TPN_SUBNET_ROOT)

from sybil.protocol import Challenge


def env_bool(name: str, default: bool = False) -> bool:
    value = os.environ.get(name)
    if value is None:
        return default
    return value.lower() == "true"


def build_config():
    parser = argparse.ArgumentParser()
    bt.Wallet.add_args(parser)
    bt.Subtensor.add_args(parser)
    bt.logging.add_args(parser)
    bt.Axon.add_args(parser)
    config = bt.Config(parser)

    if os.environ.get("BT_NETUID"):
        config.netuid = int(os.environ["BT_NETUID"])
    config.wallet.name = "tpn_pool"
    config.wallet.hotkey = "default"
    if os.environ.get("BT_WALLET_PATH"):
        config.wallet.path = os.environ["BT_WALLET_PATH"]
    if os.environ.get("BT_SUBTENSOR_NETWORK"):
        config.subtensor.network = os.environ["BT_SUBTENSOR_NETWORK"]
    if os.environ.get("BT_SUBTENSOR_CHAIN_ENDPOINT"):
        config.subtensor.chain_endpoint = os.environ["BT_SUBTENSOR_CHAIN_ENDPOINT"]
    if os.environ.get("BT_AXON_PORT"):
        config.axon.port = int(os.environ["BT_AXON_PORT"])
    if os.environ.get("BT_EXTERNAL_IP"):
        config.axon.external_ip = os.environ["BT_EXTERNAL_IP"]
    if config.blacklist is None:
        config.blacklist = bt.Config()
    config.blacklist.force_validator_permit = env_bool("BT_FORCE_VALIDATOR_PERMIT", True)
    config.blacklist.allow_non_registered = env_bool("BT_ALLOW_NON_REGISTERED", False)
    return config


class MinerShim:
    def __init__(self):
        self.config = build_config()
        bt.logging.set_config(config=self.config.logging)

        self.wallet = bt.Wallet(config=self.config)
        self.subtensor = bt.Subtensor(config=self.config)
        self.metagraph = self.subtensor.metagraph(self.config.netuid)
        self.axon = bt.Axon(wallet=self.wallet, config=self.config)
        self.backend_url = os.environ["TPN_POOL_INTERNAL_URL"].rstrip("/")
        self.axon.attach(
            forward_fn=self.forward,
            blacklist_fn=self.blacklist,
            priority_fn=self.priority,
        )

    async def forward(self, synapse: Challenge) -> Challenge:
        payload = json.dumps({"url": synapse.challenge_url}).encode("utf-8")
        req = urllib.request.Request(
            self.backend_url + "/protocol/challenge",
            data=payload,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            def do_request():
                with urllib.request.urlopen(req, timeout=180) as resp:
                    return json.loads(resp.read().decode("utf-8"))

            result = await asyncio.to_thread(do_request)
            if "response" in result:
                synapse.challenge_response = result["response"]
        except Exception as exc:
            bt.logging.error(f"forward error: {exc}")
        return synapse

    async def blacklist(self, synapse: Challenge) -> typing.Tuple[bool, str]:
        hotkey = getattr(getattr(synapse, "dendrite", None), "hotkey", None)
        if hotkey is None:
            return True, "Missing dendrite or hotkey"

        if hotkey not in self.metagraph.hotkeys:
            if self.config.blacklist.allow_non_registered:
                return False, "Allowing non-registered hotkey"
            return True, "Unrecognized hotkey"

        uid = self.metagraph.hotkeys.index(hotkey)
        if self.config.blacklist.force_validator_permit and not self.metagraph.validator_permit[uid]:
            return True, "Non-validator hotkey"

        return False, "Hotkey recognized"

    async def priority(self, synapse: Challenge) -> float:
        hotkey = getattr(getattr(synapse, "dendrite", None), "hotkey", None)
        if hotkey is None or hotkey not in self.metagraph.hotkeys:
            return 0.0
        uid = self.metagraph.hotkeys.index(hotkey)
        return float(self.metagraph.S[uid])

    def check_registered(self):
        if not self.subtensor.is_hotkey_registered(
            netuid=self.config.netuid,
            hotkey_ss58=self.wallet.hotkey.ss58_address,
        ):
            raise SystemExit(
                f"Hotkey {self.wallet.hotkey.ss58_address} is not registered on netuid {self.config.netuid}"
            )

    async def run(self):
        self.check_registered()
        self.axon.serve(netuid=self.config.netuid, subtensor=self.subtensor)
        self.axon.start()
        bt.logging.info(
            f"miner shim serving hotkey={self.wallet.hotkey.ss58_address} port={self.config.axon.port}"
        )

        try:
            while True:
                await asyncio.sleep(60)
                self.metagraph.sync(subtensor=self.subtensor)
                if self.wallet.hotkey.ss58_address not in self.metagraph.hotkeys:
                    raise SystemExit("Hotkey is no longer registered in the metagraph")
        finally:
            self.axon.stop()


if __name__ == "__main__":
    asyncio.run(MinerShim().run())
"#
}

fn env_opt(key: &str) -> Option<String> {
    env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn env_str(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_bool(key: &str) -> bool {
    env::var(key).map(|v| v == "true").unwrap_or(false)
}

fn env_bool_default(key: &str, default: bool) -> bool {
    env::var(key).map(|v| v == "true").unwrap_or(default)
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
