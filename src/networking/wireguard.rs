use crate::system::shell::run;
use tracing::info;

/// Parsed WireGuard config.
#[derive(Debug, Clone)]
pub struct ParsedWgConfig {
    pub config_valid: bool,
    pub address: Option<String>,
}

/// Parse a WireGuard config string into its components.
pub fn parse_wireguard_config(config: &str) -> ParsedWgConfig {
    if config.is_empty() {
        return ParsedWgConfig {
            config_valid: false,
            address: None,
        };
    }

    let mut endpoint = None;
    let mut public_key = None;
    let mut private_key = None;
    let mut address = None;

    for line in config.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_lowercase();
            let value = value.trim().to_string();
            match key.as_str() {
                "endpoint" => endpoint = Some(value),
                "publickey" => public_key = Some(value),
                "privatekey" => private_key = Some(value),
                "address" => address = Some(value),
                _ => {}
            }
        }
    }

    let config_valid = endpoint.is_some() && public_key.is_some() && private_key.is_some();

    ParsedWgConfig {
        config_valid,
        address,
    }
}

/// Test a WireGuard connection by setting up a namespace and curling through it.
pub async fn test_wireguard_connection(
    wireguard_config: &str,
    claimed_worker_ip: &str,
) -> WgTestResult {
    // Parse the config first
    let parsed = parse_wireguard_config(wireguard_config);
    if !parsed.config_valid {
        return WgTestResult {
            valid: false,
            message: "Invalid WireGuard config".to_string(),
            failure_code: Some("invalid_config".to_string()),
            observed_egress_ip: None,
        };
    }

    // Create a unique namespace name
    let ns_name = format!("tpn_test_{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap_or("x"));

    // Write config to temp file
    let config_path = format!("/tmp/{}.conf", ns_name);
    if let Err(e) = tokio::fs::write(&config_path, wireguard_config).await {
        return WgTestResult {
            valid: false,
            message: format!("Failed to write config: {}", e),
            failure_code: Some("write_error".to_string()),
            observed_egress_ip: None,
        };
    }

    // Set up namespace and test connection
    let test_cmd = format!(
        "ip netns add {ns} 2>/dev/null; \
         ip link add {ns} type wireguard 2>/dev/null; \
         ip link set {ns} netns {ns} 2>/dev/null; \
         ip netns exec {ns} wg setconf {ns} {config} 2>/dev/null; \
         ip netns exec {ns} ip addr add {addr} dev {ns} 2>/dev/null; \
         ip netns exec {ns} ip link set {ns} up 2>/dev/null; \
         ip netns exec {ns} ip route add default dev {ns} 2>/dev/null; \
         ip netns exec {ns} curl -s --max-time 15 https://ipv4.icanhazip.com 2>/dev/null; \
         ",
        ns = ns_name,
        config = config_path,
        addr = parsed.address.as_deref().unwrap_or("10.0.0.2/32"),
    );

    let result = run(&test_cmd, Some(20_000)).await;

    // Clean up namespace
    let _ = run(&format!("ip netns del {} 2>/dev/null; rm -f {}", ns_name, config_path), Some(5_000)).await;

    match result {
        Ok(r) if r.success => {
            let observed_ip = r.stdout.trim().to_string();
            let ip_match = observed_ip == claimed_worker_ip;

            if ip_match {
                WgTestResult {
                    valid: true,
                    message: "WireGuard connection successful".to_string(),
                    failure_code: None,
                    observed_egress_ip: Some(observed_ip),
                }
            } else {
                WgTestResult {
                    valid: false,
                    message: format!(
                        "Egress IP mismatch: observed {} vs claimed {}",
                        observed_ip, claimed_worker_ip
                    ),
                    failure_code: Some("egress_ip_mismatch".to_string()),
                    observed_egress_ip: Some(observed_ip),
                }
            }
        }
        Ok(r) => WgTestResult {
            valid: false,
            message: format!("WireGuard test failed: {}", r.stderr.trim()),
            failure_code: Some("connection_failed".to_string()),
            observed_egress_ip: None,
        },
        Err(e) => WgTestResult {
            valid: false,
            message: format!("WireGuard test error: {}", e),
            failure_code: Some("test_error".to_string()),
            observed_egress_ip: None,
        },
    }
}

#[derive(Debug)]
pub struct WgTestResult {
    pub valid: bool,
    pub message: String,
    pub failure_code: Option<String>,
    pub observed_egress_ip: Option<String>,
}

/// Clean up TPN WireGuard interfaces.
pub async fn clean_up_tpn_interfaces() {
    let _ = run("ip link show | grep 'tpn_' | awk -F: '{print $2}' | xargs -I {} ip link del {} 2>/dev/null", Some(10_000)).await;
    info!("Cleaned up TPN interfaces");
}

/// Clean up TPN network namespaces.
pub async fn clean_up_tpn_namespaces() {
    let _ = run("ip netns list | grep 'tpn_' | xargs -I {} ip netns del {} 2>/dev/null", Some(10_000)).await;
    info!("Cleaned up TPN namespaces");
}
