use crate::validations::sanitize_ipv4;

/// Extract the unspoofable IP from an axum request.
/// Uses the connection info, not headers (which can be spoofed).
pub fn ip_from_request(addr: &std::net::SocketAddr) -> String {
    sanitize_ipv4(&addr.ip().to_string())
}

/// Check if the request originates from localhost.
pub fn is_local_request(addr: &std::net::SocketAddr) -> bool {
    addr.ip().is_loopback()
}
