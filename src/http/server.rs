use crate::AppState;
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

/// Start the HTTP server with graceful shutdown.
pub async fn start_server(
    router: Router<AppState>,
    state: AppState,
    port: u16,
    shutdown_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = router.layer(cors).with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    info!("Server running on :{}", port);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(crate::system::process::shutdown_signal(shutdown_notify))
    .await?;

    Ok(())
}
