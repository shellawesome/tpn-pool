use crate::cache::TtlCache;
use serde_json::Value;
use std::sync::Arc;

/// Check if a request IP is from a known validator.
pub fn is_validator_request(ip: &str, cache: &Arc<TtlCache>) -> Option<(String, String)> {
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

    None
}

/// Get the known validators. Returns an empty list until the Python shim has
/// broadcast a neuron list (or a persisted list has been restored from the DB).
pub fn get_validators(cache: &Arc<TtlCache>) -> Vec<(String, String)> {
    let validators = cache.get_or("last_known_validators", Value::Array(vec![]));

    if let Value::Array(arr) = validators {
        return arr
            .iter()
            .filter_map(|v| {
                let uid = v.get("uid")?.as_str()?.to_string();
                let ip = v.get("ip")?.as_str()?.to_string();
                Some((uid, ip))
            })
            .collect();
    }

    Vec::new()
}
