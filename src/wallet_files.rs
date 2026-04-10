use crate::config::AppConfig;
use anyhow::{bail, Context, Result};
use bittensor_rs::crypto::Pair as _;
use bittensor_rs::Wallet;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const DEFAULT_WALLET_NAME: &str = "tpn_pool";
pub const DEFAULT_HOTKEY_NAME: &str = "default";

pub struct WalletPaths {
    pub wallet_root: PathBuf,
    pub wallet_dir: PathBuf,
    pub hotkey_path: PathBuf,
    pub coldkey_path: PathBuf,
    pub coldkeypub_path: PathBuf,
}

pub fn materialize_bittensor_wallet_from_env(config: &AppConfig) -> Result<()> {
    let hotkey_secret = hotkey_secret(config)?;
    let hotkey_wallet = wallet_from_secret(hotkey_secret, "hotkey")?;

    let wallet_dir = wallet_dir()?;
    let hotkeys_dir = wallet_dir.join("hotkeys");
    fs::create_dir_all(&hotkeys_dir)
        .with_context(|| format!("creating {}", hotkeys_dir.display()))?;
    set_dir_permissions(&wallet_dir)?;
    set_dir_permissions(&hotkeys_dir)?;

    let hotkey_path = hotkeys_dir.join(DEFAULT_HOTKEY_NAME);
    write_keyfile(&hotkey_path, hotkey_secret, &hotkey_wallet)?;

    let coldkey_secret = coldkey_secret(config)?;
    let coldkey_wallet = wallet_from_secret(coldkey_secret, "coldkey")?;
    let coldkey_path = wallet_dir.join("coldkey");
    write_keyfile(&coldkey_path, coldkey_secret, &coldkey_wallet)?;

    let coldkeypub_path = wallet_dir.join("coldkeypub.txt");
    let coldkeypub_json = json!({
        "ss58Address": coldkey_wallet.hotkey().to_string(),
        "accountId": coldkey_wallet.hotkey().to_string(),
        "publicKey": format!("0x{}", hex::encode(coldkey_wallet.keypair().public().0)),
    });
    let coldkeypub_bytes =
        serde_json::to_vec_pretty(&coldkeypub_json).context("serializing coldkeypub json")?;
    write_bytes_file(&coldkeypub_path, &coldkeypub_bytes)?;

    Ok(())
}

pub fn wallet_root() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine home directory for wallet path")?;
    Ok(home.join(".bittensor").join("wallets"))
}

pub fn wallet_dir() -> Result<PathBuf> {
    Ok(wallet_root()?.join(DEFAULT_WALLET_NAME))
}

pub fn wallet_paths() -> Result<WalletPaths> {
    let wallet_root = wallet_root()?;
    let wallet_dir = wallet_dir()?;
    Ok(WalletPaths {
        wallet_root,
        hotkey_path: wallet_dir.join("hotkeys").join(DEFAULT_HOTKEY_NAME),
        coldkey_path: wallet_dir.join("coldkey"),
        coldkeypub_path: wallet_dir.join("coldkeypub.txt"),
        wallet_dir,
    })
}

pub fn derive_hotkey_ss58(config: &AppConfig) -> Result<String> {
    let secret = hotkey_secret(config)?;
    Ok(wallet_from_secret(secret, "hotkey")?.hotkey().to_string())
}

pub fn derive_coldkey_ss58(config: &AppConfig) -> Result<String> {
    let secret = coldkey_secret(config)?;
    Ok(wallet_from_secret(secret, "coldkey")?.hotkey().to_string())
}

fn hotkey_secret(config: &AppConfig) -> Result<&str> {
    select_secret(
        config.bt_hotkey_mnemonic.as_deref(),
        config.bt_hotkey_seed_hex.as_deref(),
        "hotkey",
    )
}

fn coldkey_secret(config: &AppConfig) -> Result<&str> {
    select_secret(
        config.bt_coldkey_mnemonic.as_deref(),
        config.bt_coldkey_seed_hex.as_deref(),
        "coldkey",
    )
}

fn select_secret<'a>(
    mnemonic: Option<&'a str>,
    seed_hex: Option<&'a str>,
    label: &str,
) -> Result<&'a str> {
    match (mnemonic, seed_hex) {
        (Some(_), Some(_)) => bail!(
            "set only one of BT_{}_MNEMONIC or BT_{}_SEED_HEX",
            label.to_uppercase(),
            label.to_uppercase()
        ),
        (Some(value), None) | (None, Some(value)) => Ok(value),
        (None, None) => bail!(
            "missing {} private key: set BT_{}_MNEMONIC or BT_{}_SEED_HEX",
            label,
            label.to_uppercase(),
            label.to_uppercase()
        ),
    }
}

fn wallet_from_secret(secret: &str, label: &str) -> Result<Wallet> {
    if looks_like_hex_seed(secret) {
        Wallet::from_seed_hex(DEFAULT_WALLET_NAME, DEFAULT_HOTKEY_NAME, secret)
            .with_context(|| format!("parsing {label} seed hex"))
    } else {
        Wallet::from_mnemonic(DEFAULT_WALLET_NAME, DEFAULT_HOTKEY_NAME, secret)
            .with_context(|| format!("parsing {label} mnemonic"))
    }
}

fn write_keyfile(path: &Path, secret: &str, wallet: &Wallet) -> Result<()> {
    let key_json = if looks_like_hex_seed(secret) {
        json!({
            "secretSeed": normalized_hex_seed(secret),
            "ss58Address": wallet.hotkey().to_string(),
            "accountId": wallet.hotkey().to_string(),
            "publicKey": format!("0x{}", hex::encode(wallet.keypair().public().0)),
        })
    } else {
        json!({
            "secretPhrase": secret,
            "ss58Address": wallet.hotkey().to_string(),
            "accountId": wallet.hotkey().to_string(),
            "publicKey": format!("0x{}", hex::encode(wallet.keypair().public().0)),
        })
    };
    let bytes = serde_json::to_vec_pretty(&key_json).context("serializing keyfile json")?;
    write_bytes_file(path, &bytes)
}

fn write_bytes_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    Ok(())
}

fn set_dir_permissions(path: &Path) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    Ok(())
}

fn looks_like_hex_seed(value: &str) -> bool {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalized_hex_seed(value: &str) -> String {
    if value.starts_with("0x") {
        value.to_string()
    } else {
        format!("0x{value}")
    }
}
