use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;

// 定义对外暴露的请求 JSON 数据结构
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

// 定义返回给客户端的响应 JSON 结构
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatResponseJson {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Usage,
    pub stats: serde_json::Value,
    pub system_fingerprint: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatChoice {
    pub index: i32,
    pub logprobs: Option<serde_json::Value>,
    pub finish_reason: String,
    pub message: ChatMessageJson,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

// 为 ChatMessageJson 结构体添加 Serialize 派生宏
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessageJson {
    pub role: String,
    pub content: String,
}

// 修改 AppState 结构体，移除运行时
pub struct AppState {
    pub db: Arc<SqlitePool>,
    pub client: reqwest::Client,
    pub api_url: String,
}

// 为 temperature 提供默认值
fn default_temperature() -> f32 {
    0.1
}

// 为 max_tokens 提供默认值
fn default_max_tokens() -> i32 {
    -1
}

// 为 stream 提供默认值
fn default_stream() -> bool {
    false
}
