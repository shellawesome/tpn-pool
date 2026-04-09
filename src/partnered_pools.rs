use crate::config::AppConfig;

/// Check if a mining pool is a partnered network pool.
/// Partnered pools skip certain validation checks (version, membership).
pub fn is_partnered_pool(config: &AppConfig, mining_pool_uid: &str, mining_pool_ip: &str) -> bool {
    let identifier = format!("{}@{}", mining_pool_uid, mining_pool_ip);
    config
        .partnered_network_mining_pools
        .iter()
        .any(|pool| pool == mining_pool_uid || pool == mining_pool_ip || pool == &identifier)
}
