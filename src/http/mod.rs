pub mod routes;
pub mod server;

use crate::config::AppConfig;
use crate::dashboard;
use crate::http::routes::*;
use axum::routing::{get, post};
use axum::Router;

/// Build the axum router.
pub fn build_router(_config: &AppConfig) -> Router<crate::AppState> {
    let mut router = Router::new();

    // Health and ping
    router = router.merge(health::router());
    router = router.merge(ping::router());

    // API routes
    router = router.merge(api_lease::router());
    router = router.merge(api_status::router());

    // Protocol routes
    router = router.merge(protocol_neurons::router());
    router = router.merge(protocol_challenge::router());

    // Miner routes
    router = router.merge(miner_broadcast::router());

    // Dashboard
    router = router
        .route("/dashboard", get(dashboard::dashboard_page))
        .route("/api/login", post(dashboard::login))
        .route("/api/auth/check", get(dashboard::auth_check))
        .route("/api/dashboard", get(dashboard::dashboard_data_handler));

    router
}
