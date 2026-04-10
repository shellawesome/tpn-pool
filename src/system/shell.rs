use anyhow::Result;
use std::time::Duration;
use tokio::process::Command;
use tracing::{debug, warn};

#[derive(Debug)]
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

/// Execute a shell command with optional timeout.
pub async fn run(command: &str, timeout_ms: Option<u64>) -> Result<RunResult> {
    debug!("Running command: {}", command);

    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);

    let output = if let Some(ms) = timeout_ms {
        tokio::time::timeout(Duration::from_millis(ms), cmd.output()).await??
    } else {
        cmd.output().await?
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();
    if !success {
        debug!("Command failed: {} stderr: {}", command, stderr.trim());
    }

    Ok(RunResult {
        stdout,
        stderr,
        success,
    })
}

/// Check system resource warnings.
pub async fn check_system_warnings() {
    // Check available disk space
    if let Ok(result) = run("df -h / | tail -1 | awk '{print $5}'", Some(5000)).await {
        let usage = result.stdout.trim().replace('%', "");
        if let Ok(pct) = usage.parse::<u32>() {
            if pct > 90 {
                warn!("Disk usage is at {}%, consider freeing space", pct);
            }
        }
    }

    // Check available memory
    if let Ok(result) = run("free -m | awk '/^Mem:/{print $7}'", Some(5000)).await {
        if let Ok(available_mb) = result.stdout.trim().parse::<u64>() {
            if available_mb < 256 {
                warn!("Available memory is low: {}MB", available_mb);
            }
        }
    }
}
