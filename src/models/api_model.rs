use sqlx::SqlitePool;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::runtime::Runtime;

// 定义对外暴露的请求 JSON 数据结构
#[derive(Debug, Deserialize, Serialize)]
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
#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: i32,
    pub logprobs: Option<serde_json::Value>,
    pub finish_reason: String,
    pub message: ChatMessageJson,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: i32,
    pub completion_tokens: i32,
    pub total_tokens: i32,
}

// 为 ChatMessageJson 结构体添加 Serialize 派生宏
#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessageJson {
    pub role: String,
    pub content: String,
}

// 修改 AppState 结构体，将 db 包装在 Arc 中
pub struct AppState {
    pub db: Arc<SqlitePool>, // 使用 Arc 包装 SqlitePool
    pub client: reqwest::Client,// 用于处理 HTTP 请求
    pub api_url: String,
    pub miss_runtime: Arc<Runtime>, // 用于处理缓存未命中的运行时
    pub hit_runtime: Arc<Runtime>,  // 用于处理命中缓存的运行时
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
