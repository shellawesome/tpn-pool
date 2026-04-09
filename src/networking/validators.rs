use crate::cache::TtlCache;
use serde_json::Value;
use std::sync::Arc;

/// Hardcoded fallback validator IPs (from the Node.js source).
pub static VALIDATOR_FALLBACK_IPS: &[&str] = &[
    "194.163.159.178",
    "38.45.65.80",
    "209.145.55.229",
    "51.159.4.162",
    "209.145.50.59",
    "165.227.133.192",
];

/// Check if a request IP is from a known validator.
pub fn is_validator_request(ip: &str, cache: &Arc<TtlCache>) -> Option<(String, String)> {
    // Check the cached validator list
    let validators = cache.get_or("last_known_validators", Value::Array(vec![]));

    if let Value::Array(ref arr) = validators {
        for v in arr {
            if let Some(v_ip) = v.get("ip").and_then(|i| i.as_str()) {
                if v_ip == ip {
                    let uid = v
                        .get("uid")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    return Some((uid, ip.to_string()));
                }
            }
        }
    }

    // Check fallback IPs
    if VALIDATOR_FALLBACK_IPS.contains(&ip) {
        return Some(("fallback".to_string(), ip.to_string()));
    }

    None
}

/// Get list of known validators from cache or fallback.
pub fn get_validators(cache: &Arc<TtlCache>) -> Vec<(String, String)> {
    let validators = cache.get_or("last_known_validators", Value::Array(vec![]));

    if let Value::Array(ref arr) = validators {
        if !arr.is_empty() {
            return arr
                .iter()
                .filter_map(|v| {
                    let uid = v.get("uid")?.as_str()?.to_string();
                    let ip = v.get("ip")?.as_str()?.to_string();
                    Some((uid, ip))
                })
                .collect();
        }
    }

    // Fallback
    VALIDATOR_FALLBACK_IPS
        .iter()
        .enumerate()
        .map(|(i, ip)| (format!("fallback_{}", i), ip.to_string()))
        .collect()
}
