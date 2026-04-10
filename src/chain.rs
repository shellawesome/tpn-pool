use crate::config::AppConfig;
use crate::wallet_files::{wallet_root, DEFAULT_HOTKEY_NAME, DEFAULT_WALLET_NAME};
use anyhow::{bail, Context, Result};
use bittensor_rs::api::api;
use bittensor_rs::{config::BittensorConfig, get_metagraph, get_uid_for_hotkey, WalletSigner};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::PathBuf;
use subxt::utils::AccountId32;
use subxt::{OnlineClient, PolkadotConfig};

pub struct RegisterInfo {
    pub network: String,
    pub endpoint: String,
    pub netuid: u16,
    pub wallet_name: String,
    pub wallet_hotkey: String,
    pub wallet_path: PathBuf,
    pub hotkey_ss58: String,
    pub coldkey_ss58: String,
    pub existing_uid: Option<u16>,
}

pub async fn run_register_command(config: &AppConfig) -> Result<()> {
    let register = prepare_register(config).await?;
    print_register_info(&register.info)?;
    wait_for_register_confirmation()?;

    // burned_register extrinsic: coldkey signs + pays, hotkey is registered
    // register.hotkey_account is the exact same AccountId32 whose SS58 form was
    // shown as "Hotkey ss58" above. We re-derive and re-print its SS58 here so
    // that the value is literally taken from the field we hand to burned_register.
    let hotkey_param_ss58 = register.hotkey_account.to_string();
    println!();
    println!("==> Registering hotkey on chain: {hotkey_param_ss58}");
    if hotkey_param_ss58 != register.info.hotkey_ss58 {
        bail!(
            "internal error: hotkey ss58 displayed ({}) does not match parameter ({})",
            register.info.hotkey_ss58,
            hotkey_param_ss58
        );
    }

    let call = bittensor_rs::api::api::tx()
        .subtensor_module()
        .burned_register(register.info.netuid, register.hotkey_account.clone());

    println!("Submitting burned_register extrinsic...");
    let progress = register
        .client
        .tx()
        .sign_and_submit_then_watch_default(&call, register.coldkey_signer.keypair())
        .await
        .context("submitting burned_register extrinsic")?;

    println!("Extrinsic hash: {:?}", progress.extrinsic_hash());
    println!("Waiting for inclusion in a block (this may take ~30s)...");
    let in_block = progress
        .wait_for_finalized()
        .await
        .context("waiting for burned_register to be finalized")?;

    println!("Included in block: {:?}", in_block.block_hash());
    let events = in_block
        .wait_for_success()
        .await
        .context("burned_register extrinsic failed on-chain")?;

    println!(
        "Registration succeeded. {} events emitted.",
        events.iter().count()
    );
    Ok(())
}

/// Query the subtensor chain for the current metagraph and return the subset of
/// neurons that qualify as validators (validator_trust > 0 and a non-zero axon IP),
/// in the same `{uid, ip, validator_trust}` shape that the Python shim used to POST
/// to `/protocol/broadcast/neurons`.
pub async fn fetch_validators_from_chain(config: &AppConfig) -> Result<Vec<Value>> {
    let netuid = config
        .bt_netuid
        .context("metagraph sync requires BT_NETUID to be set")?;
    // chain_endpoint() only needs wallet/hotkey for the bittensor_rs config shim; the
    // actual storage/runtime queries below don't touch the wallet.
    let endpoint = chain_endpoint(config, DEFAULT_WALLET_NAME, DEFAULT_HOTKEY_NAME, netuid)?;
    let client = OnlineClient::<PolkadotConfig>::from_url(&endpoint)
        .await
        .with_context(|| format!("connecting to subtensor at {endpoint}"))?;

    let metagraph = get_metagraph(&client, netuid)
        .await
        .with_context(|| format!("fetching metagraph for netuid {netuid}"))?;

    let validator_trust_addr = api::storage()
        .subtensor_module()
        .validator_trust(netuid);
    let validator_trust: Vec<u16> = client
        .storage()
        .at_latest()
        .await
        .context("opening storage snapshot")?
        .fetch(&validator_trust_addr)
        .await
        .context("fetching validator_trust storage")?
        .unwrap_or_default();

    let mut out = Vec::with_capacity(metagraph.axons.len());
    for (uid, axon) in metagraph.axons.iter().enumerate() {
        let trust_raw = validator_trust.get(uid).copied().unwrap_or(0);
        if trust_raw == 0 {
            continue;
        }
        let ip = decode_axon_ip(axon.ip, axon.ip_type);
        if ip.is_empty() || ip == "0.0.0.0" || ip == "::" {
            continue;
        }
        let trust = f64::from(trust_raw) / f64::from(u16::MAX);
        out.push(json!({
            "uid": uid.to_string(),
            "ip": ip,
            "validator_trust": trust,
        }));
    }
    Ok(out)
}

fn decode_axon_ip(ip: u128, ip_type: u8) -> String {
    match ip_type {
        4 => Ipv4Addr::from(ip as u32).to_string(),
        6 => Ipv6Addr::from(ip).to_string(),
        _ => String::new(),
    }
}

pub async fn fetch_existing_uid(config: &AppConfig) -> Result<Option<u16>> {
    let netuid = config
        .bt_netuid
        .context("registration status requires BT_NETUID to be set")?;
    let wallet_name = DEFAULT_WALLET_NAME.to_string();
    let wallet_hotkey = DEFAULT_HOTKEY_NAME.to_string();
    let hotkey_signer = hotkey_signer_from_config(config)?;
    let hotkey_account = subxt::utils::AccountId32::from(hotkey_signer.public_key());
    let hotkey_ss58 = hotkey_account.to_string();
    let endpoint = chain_endpoint(config, &wallet_name, &wallet_hotkey, netuid)?;
    let client = OnlineClient::<PolkadotConfig>::from_url(&endpoint)
        .await
        .with_context(|| format!("connecting to subtensor at {endpoint}"))?;
    match get_uid_for_hotkey(&client, netuid, &hotkey_ss58).await {
        Ok(uid) => Ok(Some(uid)),
        Err(_) => Ok(None),
    }
}

struct PreparedRegister {
    info: RegisterInfo,
    client: OnlineClient<PolkadotConfig>,
    hotkey_account: AccountId32,
    coldkey_signer: WalletSigner,
}

async fn prepare_register(config: &AppConfig) -> Result<PreparedRegister> {
    let netuid = config
        .bt_netuid
        .context("register requires BT_NETUID to be set")?;
    let wallet_name = DEFAULT_WALLET_NAME.to_string();
    let wallet_hotkey = DEFAULT_HOTKEY_NAME.to_string();

    let wallet_root = wallet_root()?;
    let hotkey_signer = hotkey_signer_from_config(config)?;
    let hotkey_account = AccountId32::from(hotkey_signer.public_key());
    let hotkey_ss58 = hotkey_account.to_string();
    let endpoint = chain_endpoint(config, &wallet_name, &wallet_hotkey, netuid)?;

    let client = OnlineClient::<PolkadotConfig>::from_url(&endpoint)
        .await
        .with_context(|| format!("connecting to subtensor at {endpoint}"))?;

    let existing_uid = match get_uid_for_hotkey(&client, netuid, &hotkey_ss58).await {
        Ok(uid) => Some(uid),
        Err(_) => None,
    };

    let coldkey_signer = coldkey_signer_from_config(config)?;
    let coldkey_account = AccountId32::from(coldkey_signer.public_key());
    let coldkey_ss58 = coldkey_account.to_string();
    let info = RegisterInfo {
        network: config.bt_subtensor_network.clone(),
        endpoint,
        netuid,
        wallet_name,
        wallet_hotkey,
        wallet_path: wallet_root,
        hotkey_ss58,
        coldkey_ss58,
        existing_uid,
    };

    Ok(PreparedRegister {
        info,
        client,
        hotkey_account,
        coldkey_signer,
    })
}

fn print_register_info(info: &RegisterInfo) -> Result<()> {
    println!("TPN Pool register");
    println!();
    println!("Network: {}", info.network);
    println!("Endpoint: {}", info.endpoint);
    println!("Netuid: {}", info.netuid);
    println!("Wallet: {}", info.wallet_name);
    println!("Hotkey name: {}", info.wallet_hotkey);
    println!("Wallet path: {}", info.wallet_path.display());
    println!("Hotkey ss58: {}", info.hotkey_ss58);
    println!("Coldkey ss58: {}", info.coldkey_ss58);
    match info.existing_uid {
        Some(uid) => {
            println!("Current status: already registered as uid {uid}");
            bail!("hotkey is already registered on this subnet");
        }
        None => println!("Current status: not registered"),
    }
    Ok(())
}

fn wait_for_register_confirmation() -> Result<()> {
    print!("Press Enter to continue with burned registration, or Ctrl+C to cancel...");
    io::stdout()
        .flush()
        .context("flushing confirmation prompt")?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .context("reading confirmation input")?;
    Ok(())
}

fn coldkey_signer_from_config(config: &AppConfig) -> Result<WalletSigner> {
    let secret = if let Some(m) = &config.bt_coldkey_mnemonic {
        m.as_str()
    } else if let Some(s) = &config.bt_coldkey_seed_hex {
        s.as_str()
    } else {
        bail!("register requires BT_COLDKEY_MNEMONIC or BT_COLDKEY_SEED_HEX");
    };
    WalletSigner::from_seed(secret).map_err(|e| anyhow::anyhow!("creating coldkey signer: {e}"))
}

fn hotkey_signer_from_config(config: &AppConfig) -> Result<WalletSigner> {
    let secret = if let Some(m) = &config.bt_hotkey_mnemonic {
        m.as_str()
    } else if let Some(s) = &config.bt_hotkey_seed_hex {
        s.as_str()
    } else {
        bail!("register requires BT_HOTKEY_MNEMONIC or BT_HOTKEY_SEED_HEX");
    };
    WalletSigner::from_seed(secret).map_err(|e| anyhow::anyhow!("creating hotkey signer: {e}"))
}

fn chain_endpoint(
    config: &AppConfig,
    wallet_name: &str,
    wallet_hotkey: &str,
    netuid: u16,
) -> Result<String> {
    let mut bt_config = match config.bt_subtensor_network.as_str() {
        "finney" => BittensorConfig::finney(wallet_name, wallet_hotkey, netuid),
        "test" => BittensorConfig::testnet(wallet_name, wallet_hotkey, netuid),
        "local" => BittensorConfig::local(wallet_name, wallet_hotkey, netuid),
        other => {
            bail!(
                "unsupported BT_SUBTENSOR_NETWORK '{}'; expected finney, test, or local",
                other
            )
        }
    };
    if let Some(endpoint) = &config.bt_subtensor_chain_endpoint {
        bt_config = bt_config.with_endpoint(endpoint);
    }
    Ok(bt_config.get_chain_endpoint())
}
