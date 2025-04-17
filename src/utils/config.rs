use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::utils::cache_maintenance::CacheMaintenanceConfig;

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    pub api_endpoints: Vec<crate::models::api_model::ApiEndpoint>,
    #[serde(default = "default_use_curl")]
    pub use_curl: bool,
    #[serde(default = "default_use_proxy")]
    pub use_proxy: bool,
    #[serde(default = "default_cache_hit_pool_size")]
    pub cache_hit_pool_size: usize,
    #[serde(default = "default_cache_miss_pool_size")]
    pub cache_miss_pool_size: usize,
    #[serde(default = "default_max_concurrent_requests")]
    pub max_concurrent_requests: usize,
    #[serde(default = "default_cache_override_mode")]
    pub cache_override_mode: bool,
    #[serde(default = "default_api_headers")]
    pub api_headers: HashMap<String, String>,
    #[serde(default)]
    pub cache_maintenance: CacheMaintenanceConfig,
}

pub fn default_database_url() -> String {
    "cache.db".to_string()
}

pub fn default_use_curl() -> bool {
    false
}

pub fn default_use_proxy() -> bool {
    true
}

pub fn default_cache_hit_pool_size() -> usize {
    8
}

pub fn default_cache_miss_pool_size() -> usize {
    8
}

pub fn default_max_concurrent_requests() -> usize {
    100
}

pub fn default_cache_override_mode() -> bool {
    false
}

pub fn default_api_headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers
}

pub fn load_config() -> Result<Config, String> {
    let mut file = std::fs::File::open("config.yaml")
        .map_err(|e| format!("无法打开配置文件: {}", e))?;
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents)
        .map_err(|e| format!("无法读取配置文件: {}", e))?;
    serde_yaml::from_str(&contents)
        .map_err(|e| format!("解析配置文件失败: {}", e))
} 