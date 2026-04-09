use crate::system::shell::run;

/// Test a SOCKS5 proxy connection.
pub async fn test_socks5_connection(
    sock: &str,
    claimed_worker_ip: Option<&str>,
) -> Socks5TestResult {
    // Parse socks5://user:pass@host:port or just the connection string
    let proxy_url = if sock.starts_with("socks5://") {
        sock.to_string()
    } else {
        format!("socks5://{}", sock)
    };

    let cmd = format!(
        "curl -s --max-time 15 --proxy '{}' https://ipv4.icanhazip.com 2>/dev/null",
        proxy_url
    );

    let result = run(&cmd, Some(20_000)).await;

    match result {
        Ok(r) if r.success && !r.stdout.trim().is_empty() => {
            let observed_ip = r.stdout.trim().to_string();

            if let Some(claimed) = claimed_worker_ip {
                if observed_ip != claimed {
                    return Socks5TestResult {
                        valid: false,
                        message: format!(
                            "SOCKS5 egress IP mismatch: observed {} vs claimed {}",
                            observed_ip, claimed
                        ),
                        failure_code: Some("egress_ip_mismatch".to_string()),
                    };
                }
            }

            Socks5TestResult {
                valid: true,
                message: "SOCKS5 connection successful".to_string(),
                failure_code: None,
            }
        }
        Ok(r) => Socks5TestResult {
            valid: false,
            message: format!("SOCKS5 test failed: {}", r.stderr.trim()),
            failure_code: Some("connection_failed".to_string()),
        },
        Err(e) => Socks5TestResult {
            valid: false,
            message: format!("SOCKS5 test error: {}", e),
            failure_code: Some("test_error".to_string()),
        },
    }
}

#[derive(Debug)]
pub struct Socks5TestResult {
    pub valid: bool,
    pub message: String,
    pub failure_code: Option<String>,
}
