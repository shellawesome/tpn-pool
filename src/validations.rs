use crate::db::workers::Worker;

/// Sanitize a string by removing potentially dangerous characters.
pub fn sanitize_string(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.' || *c == '/' || *c == ':' || *c == ',' || *c == ' ')
        .collect()
}

/// Sanitize an IPv4 address (strip ::ffff: prefix).
pub fn sanitize_ipv4(ip: &str) -> String {
    ip.trim_start_matches("::ffff:").to_string()
}

/// Validate that a worker object has the minimum required fields.
pub fn is_valid_worker(worker: &Worker) -> bool {
    !worker.ip.is_empty()
        && !worker.public_port.is_empty()
        && !worker.country_code.is_empty()
}
