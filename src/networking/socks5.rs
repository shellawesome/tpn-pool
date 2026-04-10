use crate::system::shell::run;

/// Test a SOCKS5 proxy connection with retry logic matching the original implementation.
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
        "curl -S --max-time 2 --retry 3 --retry-max-time 6 --retry-delay 1 --retry-connrefused --retry-all-errors --proxy '{}' https://ipv4.icanhazip.com",
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
        Ok(r) => {
            let stderr = r.stderr.trim();
            let stdout = r.stdout.trim();
            let detail = if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                format!("stdout={}", stdout)
            } else {
                "no output".to_string()
            };

            Socks5TestResult {
                valid: false,
                message: format!("SOCKS5 test failed: {}", detail),
                failure_code: Some("socks5_connectivity_failure".to_string()),
            }
        }
        Err(e) => Socks5TestResult {
            valid: false,
            message: format!("SOCKS5 test error: {}", e),
            failure_code: Some("socks5_connectivity_failure".to_string()),
        },
    }
}

#[derive(Debug)]
pub struct Socks5TestResult {
    pub valid: bool,
    pub message: String,
    pub failure_code: Option<String>,
}
