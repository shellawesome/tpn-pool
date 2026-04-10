use std::sync::Arc;
use tokio::signal;
use tokio::sync::Notify;
use tracing::info;

/// Create a shutdown signal that listens for SIGTERM and SIGINT.
pub async fn shutdown_signal(notify: Arc<Notify>) {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => info!("Received SIGINT, shutting down"),
        _ = terminate => info!("Received SIGTERM, shutting down"),
        _ = notify.notified() => info!("Shutdown requested"),
    }
}
