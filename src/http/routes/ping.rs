use crate::networking::network::ip_from_request;
use crate::AppState;
use axum::{extract::ConnectInfo, routing::get, Router};
use std::net::SocketAddr;

pub fn router() -> Router<AppState> {
    Router::new().route("/ping", get(handler))
}

async fn handler(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> String {
    ip_from_request(&addr)
}
