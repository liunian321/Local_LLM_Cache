use crate::utils::cache_maintenance::CacheMaintenanceConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
            idle_timeout_seconds: 300,  // 默认5分钟
            check_interval_seconds: 10, // 默认10秒检查一次
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SummaryApiConfig {
    pub enabled: bool,
    pub endpoints: Vec<crate::models::api_model::ApiEndpoint>,
    pub api_key_env: String,
    pub max_tokens: i32,
    pub temperature: f32,
    pub timeout_seconds: u64,
}

impl Default for SummaryApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoints: Vec::new(),
            api_key_env: "SUMMARY_API_KEY".to_string(),
            max_tokens: 128,
            temperature: 0.2,
            timeout_seconds: 10,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContextTrimConfig {
    pub enabled: bool,
    pub max_context_tokens: usize,
    pub smart_enabled: bool,
    pub smart_max_tokens: usize,
    pub per_message_overhead: usize,
    pub min_keep_pairs: usize,
    pub summary_aggressiveness: usize,
    pub summary_mode: String,
    pub summary_api: SummaryApiConfig,
}

impl Default for ContextTrimConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_context_tokens: 4096,
            smart_enabled: false,
            smart_max_tokens: 4096,
            per_message_overhead: 3,
            min_keep_pairs: 1,
            summary_aggressiveness: 1,
            summary_mode: "local".to_string(),
            summary_api: SummaryApiConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProxyConfig {
    pub request_timeout_seconds: u64,
    pub connect_timeout_seconds: u64,
    pub response_read_timeout_seconds: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            request_timeout_seconds: 120,
            connect_timeout_seconds: 15,
            response_read_timeout_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 4321,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HttpClientConfig {
    pub timeout_seconds: u64,
    pub connect_timeout_seconds: u64,
    pub tcp_keepalive_seconds: u64,
    pub pool_idle_timeout_seconds: u64,
    pub pool_max_idle_per_host: usize,
    pub max_redirects: usize,
    pub http2_keep_alive_interval_seconds: u64,
    pub http2_keep_alive_timeout_seconds: u64,
    pub http2_initial_stream_window_size: usize,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 60,
            connect_timeout_seconds: 10,
            tcp_keepalive_seconds: 60,
            pool_idle_timeout_seconds: 180,
            pool_max_idle_per_host: 50,
            max_redirects: 5,
            http2_keep_alive_interval_seconds: 30,
            http2_keep_alive_timeout_seconds: 30,
            http2_initial_stream_window_size: 1024 * 1024, // 1MB
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub max_connections: u32,
    pub min_connections: u32,
    pub max_lifetime_seconds: u64,
    pub idle_timeout_seconds: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            min_connections: 10,
            max_lifetime_seconds: 1800, // 30 minutes
            idle_timeout_seconds: 600,  // 10 minutes
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiDefaultsConfig {
    pub default_role: String,
    pub default_object: String,
    pub default_finish_reason: String,
    pub default_system_fingerprint: String,
    pub cache_system_fingerprint: String,
    pub cache_max_size_bytes: usize,
}

impl Default for ApiDefaultsConfig {
    fn default() -> Self {
        Self {
            default_role: "assistant".to_string(),
            default_object: "chat.completion".to_string(),
            default_finish_reason: "unknown".to_string(),
            default_system_fingerprint: "unknown".to_string(),
            cache_system_fingerprint: "cached".to_string(),
            cache_max_size_bytes: 5 * 1024 * 1024, // 5MB
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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
    #[serde(default = "default_cache_version")]
    pub cache_version: u8,
    #[serde(default = "default_api_headers")]
    pub api_headers: HashMap<String, String>,
    #[serde(default)]
    pub cache_maintenance: CacheMaintenanceConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub idle_flush: IdleFlushConfig,
    #[serde(default)]
    pub context_trim: ContextTrimConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub http_client: HttpClientConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub api_defaults: ApiDefaultsConfig,
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

pub fn default_cache_version() -> u8 {
    0
}

pub fn default_api_headers() -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers
}

pub fn load_config() -> Result<Config, String> {
    let mut file =
        std::fs::File::open("config.yaml").map_err(|e| format!("无法打开配置文件: {}", e))?;
    let mut contents = String::new();
    std::io::Read::read_to_string(&mut file, &mut contents)
        .map_err(|e| format!("无法读取配置文件: {}", e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("解析配置文件失败: {}", e))
}
