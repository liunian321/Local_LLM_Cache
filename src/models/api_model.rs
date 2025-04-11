use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::{Arc, RwLock};
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ChatRequestJson {
    pub model: String,
    pub messages: Vec<ChatMessageJson>,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i32,
    #[serde(default = "default_stream")]
    pub stream: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatResponseJson {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
    #[serde(default)]
    pub stats: serde_json::Value,
    #[serde(default = "default_system_fingerprint")]
    pub system_fingerprint: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatChoice {
    pub index: i32,
    pub logprobs: Option<serde_json::Value>,
    #[serde(default = "default_finish_reason")]
    pub finish_reason: String,
    pub message: ChatMessageJson,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: i32,
    #[serde(default)]
    pub completion_tokens: i32,
    #[serde(default)]
    pub total_tokens: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessageJson {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ApiEndpoint {
    pub url: String,
    pub weight: u32,
    pub model: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<SqlitePool>,
    pub client: reqwest::Client,
    pub api_endpoints: Vec<ApiEndpoint>,
    pub max_concurrent_requests: usize,
    pub semaphore: Arc<Semaphore>,
    pub cache_version: i32,
    pub cache_override_mode: bool,
    pub use_curl: bool,
    pub use_proxy: bool,
    pub api_headers: std::collections::HashMap<String, String>,
}

fn default_system_fingerprint() -> String {
    "unknown".to_string()
}

fn default_temperature() -> f32 {
    0.1
}

fn default_max_tokens() -> i32 {
    -1
}

fn default_stream() -> bool {
    false
}

fn default_finish_reason() -> String {
    "unknown".to_string()
}
