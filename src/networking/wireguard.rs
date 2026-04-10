use crate::db::challenge_response::write_challenge_solution_pair;
use crate::db::DbPool;
use crate::system::shell::run;
use tracing::{debug, info, warn};

/// Parsed WireGuard config.
#[derive(Debug, Clone)]
pub struct ParsedWgConfig {
    pub config_valid: bool,
    pub address: Option<String>,
    pub endpoint_ipv4: Option<String>,
}

/// Parse a WireGuard config string into its components.
pub fn parse_wireguard_config(config: &str) -> ParsedWgConfig {
    if config.is_empty() {
        return ParsedWgConfig {
            config_valid: false,
            address: None,
            endpoint_ipv4: None,
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

    // Extract the IPv4 portion of the endpoint (strip :port)
    let endpoint_ipv4 = endpoint.as_ref().and_then(|e| {
        e.rsplit_once(':')
            .map(|(host, _)| host.to_string())
            .or_else(|| Some(e.clone()))
    });

    ParsedWgConfig {
        config_valid,
        address,
        endpoint_ipv4,
    }
}

/// Strip wg-quick directives and other client-side-only fields so the config is valid
/// for `wg setconf` inside the test namespace.
fn strip_wg_quick_directives(config: &str) -> String {
    let skip_keys = [
        "address",
        "dns",
        "table",
        "mtu",
        "preup",
        "postup",
        "predown",
        "postdown",
        "saveconfig",
        // ListenPort belongs to the server-side wg0; a client interface in a test
        // namespace must NOT bind it, otherwise it conflicts with port 51820.
        "listenport",
    ];
    config
        .lines()
        .filter(|line| {
            let trimmed = line.trim().to_lowercase();
            !skip_keys.iter().any(|k| {
                trimmed.starts_with(&format!("{} ", k)) || trimmed.starts_with(&format!("{}=", k))
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Generate unique per-test identifiers for the network namespace, WG interface, veth pair
/// and veth /24 subnet. All names stay under the 15-char IFNAMSIZ limit.
struct TestIds {
    ns_name: String,
    iface_name: String,
    veth_host: String,
    veth_ns: String,
    veth_subnet: String, // e.g. "10.200.37"
    tmp_config_path: String,
}

fn allocate_test_ids() -> TestIds {
    let suffix = &uuid::Uuid::new_v4().to_string()[..6];
    // Use a random octet in the 10.200.x.0/24 range to reduce collisions between
    // concurrent tests. Real collisions are handled by unique-per-test suffix anyway.
    let veth_octet = (rand::random::<u8>() % 200) + 10; // 10..210
    TestIds {
        ns_name: format!("tpn_ns_{}", suffix), // 13 chars (ns has no IFNAMSIZ limit)
        iface_name: format!("tpn_wg_{}", suffix), // 13 chars
        veth_host: format!("tpnvh_{}", suffix), // 12 chars
        veth_ns: format!("tpnvn_{}", suffix),  // 12 chars
        veth_subnet: format!("10.200.{}", veth_octet),
        tmp_config_path: format!("/tmp/tpn_wg_{}.conf", suffix),
    }
}

/// Detect the host's default outbound interface (e.g. eth0) by reading `ip route`.
async fn detect_uplink_interface() -> String {
    let result = run(
        "ip route show default | awk '{print $5; exit}'",
        Some(3_000),
    )
    .await;
    match result {
        Ok(r) if r.success => {
            let iface = r.stdout.trim().to_string();
            if !iface.is_empty() {
                return iface;
            }
        }
        _ => {}
    }
    "eth0".to_string()
}

/// Ensure the address has a /32 prefix length for `ip addr add`.
fn ensure_prefix(addr: &str) -> String {
    if addr.contains('/') {
        addr.to_string()
    } else {
        format!("{}/32", addr)
    }
}

/// Test a WireGuard connection end-to-end using a real netns + veth pair + NAT setup.
/// This matches the flow of tpn-subnet's `test_wireguard_connection`:
///   1. generate a challenge URL pointing at the pool's public HTTP endpoint
///   2. create an isolated network namespace with a veth pair to the host
///   3. set up NAT so the namespace can reach the internet via the host
///   4. bring up a WireGuard client interface inside the namespace
///   5. route the WG Endpoint through the veth (handshake packets take the veth)
///   6. route everything else through the WG interface (test traffic takes the tunnel)
///   7. curl the challenge URL through the tunnel — the request MUST traverse the
///      worker's WG server, exit to the internet, and come back to the pool's HTTP
///   8. verify the returned JSON contains the expected solution
///   9. also verify the observed egress IP matches the claimed worker IP
pub async fn test_wireguard_connection(
    wireguard_config: &str,
    claimed_worker_ip: &str,
    pool_base_url: &str,
    db: &DbPool,
) -> WgTestResult {
    // Parse the config
    let parsed = parse_wireguard_config(wireguard_config);
    if !parsed.config_valid {
        return WgTestResult::invalid_config("Missing PrivateKey, PublicKey, or Endpoint");
    }
    let endpoint_ipv4 = match parsed.endpoint_ipv4.as_deref() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return WgTestResult::invalid_config("Missing or invalid Endpoint");
        }
    };
    let tunnel_addr = ensure_prefix(parsed.address.as_deref().unwrap_or("10.0.0.2"));

    // Generate challenge/solution and store in DB so the pool HTTP endpoint can return it
    let challenge = uuid::Uuid::new_v4().to_string();
    let solution = uuid::Uuid::new_v4().to_string();
    if let Err(e) = write_challenge_solution_pair(db, &challenge, &solution) {
        return WgTestResult::test_error(format!("Failed to store challenge: {}", e));
    }
    let challenge_url = format!("{}/protocol/challenge/{}", pool_base_url, challenge);
    debug!("Challenge URL: {}", challenge_url);

    let ids = allocate_test_ids();
    let uplink = detect_uplink_interface().await;

    // Write stripped config (wg setconf rejects Address/DNS/ListenPort/etc.)
    let stripped = strip_wg_quick_directives(wireguard_config);
    if let Err(e) = tokio::fs::write(&ids.tmp_config_path, &stripped).await {
        cleanup(&ids, &uplink).await;
        return WgTestResult::test_error(format!("Failed to write config: {}", e));
    }

    // Build the full setup command. Every step is guarded with `|| exit 1` so the
    // real error surfaces. stderr is NOT suppressed so it ends up in WgTestResult.message.
    let setup_cmd = format!(
        "set -e; \
         ip netns add {ns}; \
         ip -n {ns} link set lo up; \
         ip -n {ns} link add {iface} type wireguard; \
         ip link add {vh} type veth peer name {vn}; \
         ip link set {vn} netns {ns}; \
         ip addr add {subnet}.1/24 dev {vh}; \
         ip link set {vh} up; \
         ip -n {ns} addr add {subnet}.2/24 dev {vn}; \
         ip -n {ns} link set {vn} up; \
         sysctl -q -w net.ipv4.ip_forward=1; \
         iptables -t nat -A POSTROUTING -s {subnet}.0/24 -o {uplink} -j MASQUERADE; \
         iptables -A FORWARD -i {vh} -o {uplink} -s {subnet}.0/24 -j ACCEPT; \
         iptables -A FORWARD -o {vh} -m state --state ESTABLISHED,RELATED -j ACCEPT; \
         ip netns exec {ns} wg setconf {iface} {cfg}; \
         ip -n {ns} addr add {addr} dev {iface}; \
         ip -n {ns} link set {iface} up; \
         ip -n {ns} route add default dev {iface}; \
         ip -n {ns} route add {endpoint}/32 via {subnet}.1; \
         mkdir -p /etc/netns/{ns}/; \
         echo 'nameserver 1.1.1.1' > /etc/netns/{ns}/resolv.conf",
        ns = ids.ns_name,
        iface = ids.iface_name,
        vh = ids.veth_host,
        vn = ids.veth_ns,
        subnet = ids.veth_subnet,
        uplink = uplink,
        cfg = ids.tmp_config_path,
        addr = tunnel_addr,
        endpoint = endpoint_ipv4,
    );

    match run(&setup_cmd, Some(15_000)).await {
        Ok(r) if r.success => {}
        Ok(r) => {
            let msg = format!("setup failed: {}", r.stderr.trim());
            cleanup(&ids, &uplink).await;
            return WgTestResult {
                valid: false,
                message: msg,
                failure_code: Some("setup_failed".to_string()),
                observed_egress_ip: None,
            };
        }
        Err(e) => {
            cleanup(&ids, &uplink).await;
            return WgTestResult::test_error(format!("setup error: {}", e));
        }
    }

    // Egress identity check: curl icanhazip through the tunnel, compare against claimed IP.
    // This proves the tunnel exits at the worker's public IP.
    // Give the WireGuard handshake a moment to complete before testing connectivity.
    tokio::time::sleep(std::time::Duration::from_millis(2_000)).await;

    // Use https://1.1.1.1/cdn-cgi/trace — literal IP, valid TLS cert, returns
    // `ip=<egress>` line. Avoids depending on DNS inside the test namespace.
    let egress_cmd = format!(
        "ip netns exec {} curl -sS --max-time 10 https://1.1.1.1/cdn-cgi/trace 2>&1",
        ids.ns_name
    );
    let egress_result = run(&egress_cmd, Some(15_000)).await;

    // Capture diagnostics up front so we can include them in any failure message.
    let wg_show = run(
        &format!("ip netns exec {} wg show {}", ids.ns_name, ids.iface_name),
        Some(3_000),
    )
    .await
    .ok()
    .map(|r| r.stdout.trim().to_string())
    .unwrap_or_default();

    // Host-side UDP reachability test: try to send a dummy UDP packet directly
    // from the host default namespace to the WG endpoint. If this fails at the
    // host level, we know the pool host itself can't reach the worker's 51820/udp
    // and the netns is not at fault.
    let host_udp_probe = run(
        &format!(
            "timeout 2 bash -c 'exec 3<>/dev/udp/{}/51820 && printf probe >&3 && echo ok || echo fail'",
            endpoint_ipv4
        ),
        Some(3_000),
    )
    .await
    .ok()
    .map(|r| format!("{} {}", r.stdout.trim(), r.stderr.trim()))
    .unwrap_or_default();

    // Dump relevant iptables rules and the netns routing table to help diagnose
    // why the reply packets aren't coming back.
    let netns_route = run(&format!("ip -n {} route", ids.ns_name), Some(2_000))
        .await
        .ok()
        .map(|r| r.stdout.trim().replace('\n', " | "))
        .unwrap_or_default();
    let ipt_forward = run("iptables -S FORWARD", Some(2_000))
        .await
        .ok()
        .map(|r| r.stdout.trim().replace('\n', " | "))
        .unwrap_or_default();
    let ipt_nat = run("iptables -t nat -S POSTROUTING", Some(2_000))
        .await
        .ok()
        .map(|r| r.stdout.trim().replace('\n', " | "))
        .unwrap_or_default();
    let diag = format!(
        "host_udp_probe={} | nsroute={} | fwd={} | nat={}",
        host_udp_probe, netns_route, ipt_forward, ipt_nat
    );

    let observed_egress_ip = match egress_result {
        Ok(r) if r.success => {
            // Extract `ip=X.X.X.X` from the trace output.
            let ip = r
                .stdout
                .lines()
                .find_map(|l| l.trim().strip_prefix("ip=").map(str::to_string))
                .unwrap_or_default();
            if ip.is_empty() {
                cleanup(&ids, &uplink).await;
                return WgTestResult {
                    valid: false,
                    message: format!(
                        "Egress check returned no IP. Output: {}. wg: {}",
                        r.stdout.trim(),
                        wg_show
                    ),
                    failure_code: Some("no_egress_ip".to_string()),
                    observed_egress_ip: None,
                };
            }
            ip
        }
        Ok(r) => {
            let combined = format!("{} {}", r.stdout.trim(), r.stderr.trim());
            let msg = format!(
                "Egress check failed: {}. wg: {} | diag: {}",
                combined.trim(),
                wg_show,
                diag
            );
            cleanup(&ids, &uplink).await;
            return WgTestResult {
                valid: false,
                message: msg,
                failure_code: Some("wireguard_connectivity_failure".to_string()),
                observed_egress_ip: None,
            };
        }
        Err(e) => {
            cleanup(&ids, &uplink).await;
            return WgTestResult::test_error(format!("egress check error: {}. wg: {}", e, wg_show));
        }
    };

    if observed_egress_ip != claimed_worker_ip {
        cleanup(&ids, &uplink).await;
        return WgTestResult {
            valid: false,
            message: format!(
                "Egress IP mismatch: observed {} vs claimed {}",
                observed_egress_ip, claimed_worker_ip
            ),
            failure_code: Some("egress_ip_mismatch".to_string()),
            observed_egress_ip: Some(observed_egress_ip),
        };
    }

    // Send the challenge through the tunnel. The request traverses the worker's
    // WG server out to the internet, then back to the pool's HTTP endpoint.
    let challenge_cmd = format!(
        "ip netns exec {} curl -sS --max-time 15 {}",
        ids.ns_name, challenge_url
    );
    let challenge_response = match run(&challenge_cmd, Some(20_000)).await {
        Ok(r) if r.success => r.stdout,
        Ok(r) => {
            let msg = format!(
                "Challenge curl failed: {} {}",
                r.stdout.trim(),
                r.stderr.trim()
            );
            cleanup(&ids, &uplink).await;
            return WgTestResult {
                valid: false,
                message: msg,
                failure_code: Some("challenge_failure".to_string()),
                observed_egress_ip: Some(observed_egress_ip),
            };
        }
        Err(e) => {
            cleanup(&ids, &uplink).await;
            return WgTestResult::test_error(format!("challenge curl error: {}", e));
        }
    };

    cleanup(&ids, &uplink).await;

    // Parse JSON response and verify solution
    let json: serde_json::Value = match serde_json::from_str(challenge_response.trim()) {
        Ok(v) => v,
        Err(e) => {
            return WgTestResult {
                valid: false,
                message: format!(
                    "Invalid JSON from challenge endpoint: {} (body: {})",
                    e,
                    challenge_response.trim()
                ),
                failure_code: Some("challenge_failure".to_string()),
                observed_egress_ip: Some(observed_egress_ip),
            };
        }
    };
    let responded_solution = json.get("solution").and_then(|v| v.as_str()).unwrap_or("");
    if responded_solution != solution {
        return WgTestResult {
            valid: false,
            message: format!(
                "Challenge solution mismatch: expected {}, got {}",
                solution, responded_solution
            ),
            failure_code: Some("challenge_failure".to_string()),
            observed_egress_ip: Some(observed_egress_ip),
        };
    }

    info!(
        "WireGuard validation passed for {} (egress: {})",
        claimed_worker_ip, observed_egress_ip
    );
    WgTestResult {
        valid: true,
        message: format!(
            "WireGuard connection successful (egress {})",
            observed_egress_ip
        ),
        failure_code: None,
        observed_egress_ip: Some(observed_egress_ip),
    }
}

/// Tear down the netns, veth pair, iptables rules, and temp config.
/// All steps are best-effort — individual failures are logged but don't block.
async fn cleanup(ids: &TestIds, uplink: &str) {
    let cmd = format!(
        "ip link del {vh} 2>/dev/null; \
         ip link del {iface} 2>/dev/null; \
         ip netns del {ns} 2>/dev/null; \
         iptables -t nat -D POSTROUTING -s {subnet}.0/24 -o {uplink} -j MASQUERADE 2>/dev/null; \
         iptables -D FORWARD -i {vh} -o {uplink} -s {subnet}.0/24 -j ACCEPT 2>/dev/null; \
         iptables -D FORWARD -o {vh} -m state --state ESTABLISHED,RELATED -j ACCEPT 2>/dev/null; \
         rm -rf /etc/netns/{ns} 2>/dev/null; \
         rm -f {cfg} 2>/dev/null; \
         true",
        ns = ids.ns_name,
        iface = ids.iface_name,
        vh = ids.veth_host,
        subnet = ids.veth_subnet,
        uplink = uplink,
        cfg = ids.tmp_config_path,
    );
    if let Err(e) = run(&cmd, Some(5_000)).await {
        warn!("Cleanup error for ns {}: {}", ids.ns_name, e);
    }
}

#[derive(Debug)]
pub struct WgTestResult {
    pub valid: bool,
    pub message: String,
    pub failure_code: Option<String>,
    pub observed_egress_ip: Option<String>,
}

impl WgTestResult {
    fn invalid_config(msg: impl Into<String>) -> Self {
        Self {
            valid: false,
            message: format!("Invalid WireGuard config: {}", msg.into()),
            failure_code: Some("invalid_config".to_string()),
            observed_egress_ip: None,
        }
    }
    fn test_error(msg: impl Into<String>) -> Self {
        Self {
            valid: false,
            message: format!("WireGuard test error: {}", msg.into()),
            failure_code: Some("test_error".to_string()),
            observed_egress_ip: None,
        }
    }
}

/// Clean up any leftover TPN WireGuard interfaces (called on startup/shutdown).
pub async fn clean_up_tpn_interfaces() {
    let _ = run(
        "ip link show | grep -oE 'tpn_wg_[a-f0-9]+|tpnvh_[a-f0-9]+|tpnvn_[a-f0-9]+' \
         | xargs -I {} ip link del {} 2>/dev/null; true",
        Some(10_000),
    )
    .await;
    info!("Cleaned up TPN interfaces");
}

/// Clean up any leftover TPN network namespaces (called on startup/shutdown).
pub async fn clean_up_tpn_namespaces() {
    let _ = run(
        "ip netns list | grep -oE 'tpn_ns_[a-f0-9]+' \
         | xargs -I {} sh -c 'ip netns del {} 2>/dev/null; rm -rf /etc/netns/{}'; true",
        Some(10_000),
    )
    .await;
    info!("Cleaned up TPN namespaces");
}
