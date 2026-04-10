use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use tracing::debug;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseTokenPayload {
    pub config_ref: String,
    #[serde(rename = "type")]
    pub lease_type: String,
    pub worker_ip: String,
    pub mining_pool_url: String,
    pub mining_pool_uid: String,
    pub expires_at: i64,
}

/// Signs a lease token payload into a base64url-encoded opaque string.
/// Format: `base64url(json_payload).base64url(hmac_sha256_signature)`
pub fn sign_lease_token(secret: &str, payload: &LeaseTokenPayload) -> String {
    let json = serde_json::to_string(payload).expect("failed to serialize lease token payload");
    let payload_b64 = URL_SAFE_NO_PAD.encode(json.as_bytes());

    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload_b64.as_bytes());
    let signature = mac.finalize().into_bytes();
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature);

    debug!(
        "Signed lease token for config_ref={} type={} worker={}",
        payload.config_ref, payload.lease_type, payload.worker_ip
    );

    format!("{}.{}", payload_b64, signature_b64)
}

/// Verifies and decodes a signed lease token.
/// Uses timing-safe comparison to prevent side-channel attacks.
pub fn verify_lease_token(secret: &str, token: &str) -> Result<LeaseTokenPayload> {
    let dot_index = token
        .rfind('.')
        .ok_or_else(|| anyhow!("Invalid lease token: missing signature separator"))?;

    let payload_b64 = &token[..dot_index];
    let provided_sig_b64 = &token[dot_index + 1..];

    // Recompute HMAC
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload_b64.as_bytes());
    let expected_sig = mac.finalize().into_bytes();
    let expected_sig_b64 = URL_SAFE_NO_PAD.encode(expected_sig);

    // Timing-safe comparison
    let provided_bytes = provided_sig_b64.as_bytes();
    let expected_bytes = expected_sig_b64.as_bytes();
    if provided_bytes.len() != expected_bytes.len()
        || provided_bytes.ct_eq(expected_bytes).unwrap_u8() != 1
    {
        return Err(anyhow!("Invalid lease token: signature mismatch"));
    }

    // Decode payload
    let payload_json = URL_SAFE_NO_PAD.decode(payload_b64)?;
    let payload: LeaseTokenPayload = serde_json::from_slice(&payload_json)?;

    debug!(
        "Verified lease token for config_ref={} type={}",
        payload.config_ref, payload.lease_type
    );

    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_and_verify_roundtrip() {
        let secret = "test-secret-key-for-lease-tokens";
        let payload = LeaseTokenPayload {
            config_ref: "42".to_string(),
            lease_type: "wireguard".to_string(),
            worker_ip: "1.2.3.4".to_string(),
            mining_pool_url: "http://pool.example.com:3000".to_string(),
            mining_pool_uid: "internal".to_string(),
            expires_at: 1700000000,
        };

        let token = sign_lease_token(secret, &payload);
        assert!(token.contains('.'));

        let decoded = verify_lease_token(secret, &token).unwrap();
        assert_eq!(decoded.config_ref, "42");
        assert_eq!(decoded.lease_type, "wireguard");
        assert_eq!(decoded.worker_ip, "1.2.3.4");
        assert_eq!(decoded.mining_pool_url, "http://pool.example.com:3000");
        assert_eq!(decoded.mining_pool_uid, "internal");
        assert_eq!(decoded.expires_at, 1700000000);
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let secret = "test-secret";
        let payload = LeaseTokenPayload {
            config_ref: "1".to_string(),
            lease_type: "socks5".to_string(),
            worker_ip: "5.6.7.8".to_string(),
            mining_pool_url: "http://pool:3000".to_string(),
            mining_pool_uid: "uid".to_string(),
            expires_at: 123456,
        };

        let token = sign_lease_token(secret, &payload);
        let result = verify_lease_token("wrong-secret", &token);
        assert!(result.is_err());
    }

    #[test]
    fn test_malformed_token_rejected() {
        let result = verify_lease_token("secret", "no-dot-in-this-token");
        // Has a dot from base64 potentially but let's test truly malformed
        let result2 = verify_lease_token("secret", "abc.def");
        assert!(result.is_err() || result2.is_err());
    }
}
