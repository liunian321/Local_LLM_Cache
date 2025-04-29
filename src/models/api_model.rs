use rand::prelude::*;
use rand_distr::weighted::WeightedIndex;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Semaphore;
use crate::utils::memory_cache::MemoryCache;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_thinking: Option<bool>,
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
    pub weight: u8,
    pub model: Option<String>,
    #[serde(default = "default_version")]
    pub version: u8,
}

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<SqlitePool>,
    pub client: reqwest::Client,
    pub api_endpoints: Vec<ApiEndpoint>,
    pub max_concurrent_requests: usize,
    pub semaphore: Arc<Semaphore>,
    pub cache_override_mode: bool,
    pub use_curl: bool,
    pub use_proxy: bool,
    pub enable_thinking: Option<bool>,
    pub api_headers: std::collections::HashMap<String, String>,
    pub memory_cache: Option<Arc<MemoryCache>>,
    pub cache_enabled: bool,
    pub batch_write_size: usize,
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

fn default_version() -> u8 {
    0
}

pub fn select_api_endpoint(endpoints: &[ApiEndpoint]) -> Option<ApiEndpoint> {
    if endpoints.is_empty() {
        return None;
    }

    let valid_endpoints: Vec<&ApiEndpoint> = endpoints
        .iter()
        .filter(|endpoint| endpoint.weight > 0)
        .collect();

    if valid_endpoints.is_empty() {
        return Some(endpoints[0].clone());
    }

    let weights: Vec<u8> = valid_endpoints.iter().map(|ep| ep.weight).collect();

    let mut rng = rand::rng();

    match WeightedIndex::new(&weights) {
        Ok(dist) => {
            let chosen_index = dist.sample(&mut rng);
            Some((*valid_endpoints[chosen_index]).clone())
        }
        Err(_) => Some((*valid_endpoints[0]).clone()),
    }
}
