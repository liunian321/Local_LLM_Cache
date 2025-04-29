use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::utils::cache_maintenance::CacheMaintenanceConfig;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CacheConfig {
    pub enabled: bool,
    pub max_items: usize,
    pub batch_write_size: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: 100,
            batch_write_size: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IdleFlushConfig {
    pub enabled: bool,
    pub idle_timeout_seconds: u64,
    pub check_interval_seconds: u64,
}

impl Default for IdleFlushConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_timeout_seconds: 300, // 默认5分钟
            check_interval_seconds: 10, // 默认10秒检查一次
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    pub api_endpoints: Vec<crate::models::api_model::ApiEndpoint>,
    #[serde(default = "default_use_curl")]
    pub use_curl: bool,
    #[serde(default = "default_use_proxy")]
    pub use_proxy: bool,
    #[serde(default)]
    pub enable_thinking: Option<bool>,
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
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub idle_flush: IdleFlushConfig,
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