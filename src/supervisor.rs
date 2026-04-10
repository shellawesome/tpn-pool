use crate::config::AppConfig;
use crate::wallet_files::{wallet_root, DEFAULT_HOTKEY_NAME, DEFAULT_WALLET_NAME};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{error, info, warn};

pub struct PythonShimProcess {
    child: Child,
}

impl PythonShimProcess {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub async fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child.wait().await.context("waiting for python shim")
    }
}

pub fn should_start_python_shim(config: &AppConfig) -> bool {
    config.python_shim_enabled
}

pub fn validate_python_shim_config(config: &AppConfig) -> Result<()> {
    if !config.python_shim_enabled {
        return Ok(());
    }

    if config.bt_netuid.is_none() {
        bail!("PYTHON_SHIM_ENABLED=true requires BT_NETUID");
    }
    if config.bt_hotkey_mnemonic.is_none() && config.bt_hotkey_seed_hex.is_none() {
        bail!("PYTHON_SHIM_ENABLED=true requires BT_HOTKEY_MNEMONIC or BT_HOTKEY_SEED_HEX");
    }
    if config.bt_coldkey_mnemonic.is_none() && config.bt_coldkey_seed_hex.is_none() {
        bail!("PYTHON_SHIM_ENABLED=true requires BT_COLDKEY_MNEMONIC or BT_COLDKEY_SEED_HEX");
    }
    Ok(())
}

pub async fn verify_python_shim_environment(config: &AppConfig) -> Result<()> {
    validate_python_shim_config(config)?;
    ensure_path_exists(&config.python_shim_path)?;

    ensure_python_root_is_usable(&config.sybil_python_root)?;
    verify_wallet_layout(config)?;
    verify_python_binary(&config.python_bin).await?;
    verify_python_import(&config.python_bin, "import bittensor").await?;
    verify_sybil_protocol_import(&config.python_bin, &config.sybil_python_root).await?;
    Ok(())
}

pub async fn wait_for_backend_ready(port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/ping");
    for _ in 0..50 {
        match reqwest::get(&url).await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    }

    bail!("backend did not become ready on {url}")
}

pub async fn spawn_python_shim(config: &AppConfig) -> Result<PythonShimProcess> {
    validate_python_shim_config(config)?;
    ensure_path_exists(&config.python_shim_path)?;

    let mut command = Command::new(&config.python_bin);
    command
        .arg(&config.python_shim_path)
        .env(
            "TPN_POOL_INTERNAL_URL",
            format!("http://127.0.0.1:{}", config.server_port),
        )
        .env("TPN_SUBNET_PYTHON_ROOT", &config.sybil_python_root)
        .env(
            "BT_NETUID",
            config.bt_netuid.expect("validated above").to_string(),
        )
        .env("BT_SUBTENSOR_NETWORK", config.bt_subtensor_network.as_str())
        .env("BT_AXON_PORT", config.bt_axon_port.to_string())
        .env(
            "BT_FORCE_VALIDATOR_PERMIT",
            bool_string(config.bt_force_validator_permit),
        )
        .env(
            "BT_ALLOW_NON_REGISTERED",
            bool_string(config.bt_allow_non_registered),
        )
        .env("BT_WALLET_NAME", DEFAULT_WALLET_NAME)
        .env("BT_WALLET_HOTKEY", DEFAULT_HOTKEY_NAME)
        .env("BT_WALLET_PATH", wallet_root()?.display().to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(external_ip) = &config.bt_external_ip {
        command.env("BT_EXTERNAL_IP", external_ip);
    }
    if let Some(endpoint) = &config.bt_subtensor_chain_endpoint {
        command.env("BT_SUBTENSOR_CHAIN_ENDPOINT", endpoint);
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawning python shim with {}", config.python_bin))?;

    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(stream_logs(stdout, "python-shim", false));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(stream_logs(stderr, "python-shim", true));
    }

    Ok(PythonShimProcess { child })
}

async fn stream_logs<R>(reader: R, source: &'static str, is_stderr: bool)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if is_stderr {
            warn!(target: "python_shim", source, "{line}");
        } else {
            info!(target: "python_shim", source, "{line}");
        }
    }
}

fn ensure_path_exists(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    bail!("python shim script does not exist at {}", path.display())
}

fn ensure_python_root_is_usable(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!(
            "TPN_SUBNET_PYTHON_ROOT does not exist or is not a directory: {}",
            path.display()
        );
    }

    let sybil_protocol = path.join("sybil").join("protocol.py");
    if !sybil_protocol.is_file() {
        bail!(
            "TPN_SUBNET_PYTHON_ROOT is missing sybil/protocol.py: {}",
            sybil_protocol.display()
        );
    }

    Ok(())
}

fn verify_wallet_layout(_config: &AppConfig) -> Result<()> {
    let wallet_name = DEFAULT_WALLET_NAME;
    let hotkey_name = DEFAULT_HOTKEY_NAME;
    let wallet_root = wallet_root()?;
    if !wallet_root.is_dir() {
        bail!(
            "wallet root does not exist or is not a directory: {}",
            wallet_root.display()
        );
    }

    let wallet_dir = wallet_root.join(wallet_name);
    if !wallet_dir.is_dir() {
        bail!("wallet directory does not exist: {}", wallet_dir.display());
    }

    let hotkey_file = wallet_dir.join("hotkeys").join(hotkey_name);
    if !hotkey_file.is_file() {
        bail!(
            "wallet hotkey file does not exist: {}",
            hotkey_file.display()
        );
    }

    let coldkeypub_file = wallet_dir.join("coldkeypub.txt");
    if !coldkeypub_file.is_file() {
        bail!(
            "wallet coldkeypub file does not exist: {}",
            coldkeypub_file.display()
        );
    }

    Ok(())
}

async fn verify_python_binary(python_bin: &str) -> Result<()> {
    let output = Command::new(python_bin)
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("failed to execute Python interpreter '{python_bin}'"))?;

    if !output.status.success() {
        bail!(
            "python interpreter '{}' is not usable: {}",
            python_bin,
            stderr_or_stdout(&output)
        );
    }

    Ok(())
}

async fn verify_python_import(python_bin: &str, script: &str) -> Result<()> {
    let output = Command::new(python_bin)
        .arg("-c")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("failed to execute Python import check with '{python_bin}'"))?;

    if !output.status.success() {
        bail!(
            "python dependency check failed for '{}': {}",
            script,
            stderr_or_stdout(&output)
        );
    }

    Ok(())
}

async fn verify_sybil_protocol_import(python_bin: &str, python_root: &Path) -> Result<()> {
    let python_root_literal = serde_json::to_string(&python_root.display().to_string())
        .context("serializing TPN_SUBNET_PYTHON_ROOT for Python import check")?;
    let script = format!(
        "import sys; sys.path.insert(0, {}); import sybil.protocol",
        python_root_literal
    );
    verify_python_import(python_bin, &script).await
}

fn stderr_or_stdout(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }

    "process exited without output".to_string()
}

fn bool_string(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

pub async fn supervise_python_shim(config: AppConfig) -> Result<()> {
    verify_python_shim_environment(&config).await?;
    let restart_delay = std::time::Duration::from_secs(config.python_shim_restart_delay_seconds);
    let mut restart_count = 0_u32;

    loop {
        let mut shim = spawn_python_shim(&config).await?;
        info!(
            pid = shim.id().unwrap_or_default(),
            script = %config.python_shim_path.display(),
            "python shim started"
        );

        let status = shim.wait().await?;
        if status.success() {
            info!("python shim exited cleanly");
            return Ok(());
        }

        restart_count += 1;
        error!(
            code = status.code().unwrap_or(-1),
            restart_count, "python shim exited unexpectedly"
        );

        if restart_count >= 3 {
            bail!("python shim crashed too many times");
        }

        tokio::time::sleep(restart_delay).await;
    }
}
