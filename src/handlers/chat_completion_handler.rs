use crate::handlers::api_handler::send_request_with_curl;
use crate::models::api_model::{
    AppState, ChatChoice, ChatMessageJson, ChatRequestJson, ChatResponseJson, Usage,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use brotli::CompressorWriter;
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::env;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::{oneshot, Semaphore};
use uuid::Uuid;

pub type TaskSender = tokio::sync::mpsc::Sender<BoxFuture<'static, ()>>;

// 缓存查询的异步函数
async fn query_cache(
    db: Arc<sqlx::SqlitePool>,
    cache_key: String,
    cache_version: i32,
    cache_override_mode: bool,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    println!("并行查询缓存: {}", &cache_key[..8]); // 只显示key的前8位

    let result = if cache_override_mode {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ? AND version = ?")
            .bind(cache_key.clone())
            .bind(cache_version)
            .fetch_optional(&*db)
            .await?
    } else {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ?")
            .bind(cache_key.clone())
            .fetch_optional(&*db)
            .await?
    };

    Ok(result.map(|(data,)| data))
}

// 处理解压缩缓存内容
async fn process_cached_response(
    compressed_data: Vec<u8>,
    payload: ChatRequestJson,
) -> Result<Json<ChatResponseJson>, (StatusCode, String)> {
    let mut decompressed = Vec::new();
    let mut decompressor = brotli::Decompressor::new(compressed_data.as_slice(), compressed_data.len());

    match std::io::copy(&mut decompressor, &mut decompressed) {
        Ok(_) => match String::from_utf8(decompressed) {
            Ok(message_content) => {
                // 构建响应
                let response = ChatResponseJson {
                    id: Uuid::new_v4().to_string(),
                    object: "chat.completion".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    model: payload.model.clone(),
                    choices: vec![ChatChoice {
                        index: 0,
                        logprobs: None,
                        finish_reason: "stop_from_cache".to_string(),
                        message: ChatMessageJson {
                            role: "assistant".to_string(),
                            content: message_content,
                        },
                    }],
                    usage: Usage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: 0,
                    },
                    stats: serde_json::Value::Null,
                    system_fingerprint: "cached".to_string(),
                };

                println!("缓存命中");
                Ok(Json(response))
            }
            Err(e) => Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("解析缓存内容失败: {}", e),
            )),
        },
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("解压缩缓存数据失败: {}", e),
        )),
    }
}

// 发送API请求
async fn send_api_request(
    client: reqwest::Client,
    target_url: String,
    payload_json: String,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    println!("发送上游API请求");
    
    // 持有许可直到函数返回
    let _permit = permit;
    
    // 构建请求
    let request_builder = client
        .post(&target_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "llm_api_rust_client/1.0");

    // 发送请求
    let response = match request_builder.body(payload_json.clone()).send().await {
        Ok(res) => res,
        Err(e) => {
            // 尝试使用curl作为备选
            let use_curl: bool = env::var("USE_CURL")
                .unwrap_or_else(|_| "false".to_string())
                .parse::<bool>()
                .unwrap_or(false);
                
            if use_curl {
                return send_request_with_curl(&target_url, &payload_json).await;
            } else {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("请求上游API失败: {}", e),
                ));
            }
        }
    };

    // 处理响应
    let status = response.status();
    if !status.is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "无法读取错误响应体".to_string());
        return Err((
            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            format!("上游API返回错误: 状态码 = {}, 内容 = {}", status, error_body),
        ));
    }

    // 读取响应体
    let text = match response.text().await {
        Ok(text) => text,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("读取上游响应失败: {}", e),
            ));
        }
    };

    // 解析响应体
    match serde_json::from_str::<ChatResponseJson>(&text) {
        Ok(json) => Ok(json),
        Err(e) => {
            eprintln!("解析上游响应失败: {}, Body: {}", e, text);
            Err((StatusCode::INTERNAL_SERVER_ERROR, "处理上游响应失败".to_string()))
        }
    }
}

// chat_completion
#[axum::debug_handler]
pub async fn chat_completion(
    State(app_state): State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
    Json(payload): Json<ChatRequestJson>,
) -> Response {
    let (state, tx_hit, tx_miss) = {
        let (state_ref, tx_hit_ref, tx_miss_ref) = &*app_state;
        (state_ref.clone(), tx_hit_ref.clone(), tx_miss_ref.clone())
    };

    // 从环境变量读取配置
    let cache_version: i32 = match env::var("CACHE_VERSION")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
    {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("解析缓存版本失败: {}", e),
            )
                .into_response();
        }
    };

    let cache_override_mode: bool = match env::var("CACHE_OVERRIDE_MODE")
        .unwrap_or_else(|_| "false".to_string())
        .parse()
    {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("解析覆盖模式失败: {}", e),
            )
                .into_response();
        }
    };

    // 提取用户消息并计算缓存键
    let user_message = match payload
        .messages
        .iter()
        .find(|msg| msg.role.to_lowercase() == "user")
    {
        Some(msg) => msg,
        None => return (StatusCode::BAD_REQUEST, "未找到用户消息").into_response(),
    };

    let mut hasher = Sha256::new();
    hasher.update(user_message.content.as_bytes());
    let cache_key = hex::encode(hasher.finalize());

    // 处理主线程，同时实现两路处理
    // 1. 创建响应通道
    let (response_tx, response_rx) = oneshot::channel::<Result<Json<ChatResponseJson>, (StatusCode, String)>>();
    
    // 2. 开启并发处理
    tokio::spawn(async move {
        // 查询缓存（异步操作）
        let cache_result = query_cache(
            state.db.clone(),
            cache_key.clone(), 
            cache_version,
            cache_override_mode
        ).await;
        
        match cache_result {
            Ok(Some(compressed_data)) => {
                // 命中缓存，处理缓存数据
                let hit_task = Box::pin(async move {
                    let result = process_cached_response(compressed_data, payload.clone()).await;
                    // 发送结果回调用者
                    let _ = response_tx.send(result);
                });
                
                // 发送到缓存命中处理线程池
                let _ = tx_hit.send(hit_task).await;
            },
            Ok(None) => {
                // 缓存未命中，准备API请求
                // 创建信号量
                let semaphore = Arc::new(Semaphore::new(state.max_concurrent_requests));
                
                // 准备API URL
                let api_url = state.api_url.clone();
                let target_url = if api_url.ends_with('/') {
                    format!("{}v1/chat/completions", api_url)
                } else {
                    format!("{}/v1/chat/completions", api_url)
                };
                
                // 序列化请求负载
                let payload_json = match serde_json::to_string(&payload) {
                    Ok(json) => json,
                    Err(e) => {
                        let _ = response_tx.send(Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("序列化请求负载失败: {}", e),
                        )));
                        return;
                    }
                };
                
                // 获取信号量许可
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(permit) => permit,
                    Err(e) => {
                        eprintln!("获取信号量失败: {}", e);
                        let _ = response_tx.send(Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "获取并发许可失败".to_string(),
                        )));
                        return;
                    }
                };
                
                // 创建API请求任务
                let miss_task = Box::pin(async move {
                    // 发送API请求并获取结果
                    let api_result = send_api_request(
                        state.client.clone(),
                        target_url,
                        payload_json,
                        permit
                    ).await;
                    
                    match &api_result {
                        Ok(response_json) => {
                            // 在后台缓存结果
                            let response_clone = response_json.clone();
                            let db_clone = state.db.clone();
                            tokio::spawn(async move {
                                cache_response(response_clone, cache_key, db_clone, cache_version).await;
                            });
                            
                            // 返回结果
                            let _ = response_tx.send(Ok(Json(response_json.clone())));
                        },
                        Err((status, message)) => {
                            let _ = response_tx.send(Err((status.clone(), message.clone())));
                        }
                    }
                });
                
                // 发送到未命中缓存处理线程池
                let _ = tx_miss.send(miss_task).await;
            },
            Err(e) => {
                // 数据库查询错误
                let _ = response_tx.send(Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("数据库查询错误: {}", e),
                )));
            }
        }
    });

    // 等待响应结果
    match response_rx.await {
        Ok(result) => result.into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "处理请求失败").into_response(),
    }
}

// 新增的缓存响应函数
async fn cache_response(
    response_json: ChatResponseJson, 
    cache_key: String, 
    db: Arc<sqlx::SqlitePool>, 
    cache_version: i32
) {
    // 检查 choices 和 message content
    if response_json.choices.is_empty() {
        eprintln!("上游 API 返回的 choices 数组为空，跳过缓存");
        return;
    }
    
    let message_content = &response_json.choices[0].message.content;
    if message_content.is_empty() {
        eprintln!("上游 API 返回的 message 内容为空，跳过缓存");
        return;
    }

    // 压缩消息内容
    let message_bytes = message_content.as_bytes();
    let mut compressed = Vec::new();
    {
        let mut compressor = CompressorWriter::new(&mut compressed, 4096, 11, 22);
        if let Err(e) = compressor.write_all(message_bytes) {
            eprintln!("压缩响应失败: {}", e);
            return;
        }
        if let Err(e) = compressor.flush() {
            eprintln!("刷新压缩器失败: {}", e);
            return;
        }
    }

    let data_size = compressed.len() as i64;

    // 缓存响应
    let insert_result = sqlx::query(
        "INSERT OR REPLACE INTO cache (key, response, size, hit_count, version) VALUES (?, ?, ?, 0, ?)"
    )
    .bind(cache_key.clone())
    .bind(compressed)
    .bind(data_size)
    .bind(cache_version)
    .execute(&*db)
    .await;

    match insert_result {
        Ok(_) => {
            println!("成功缓存响应 Size: {}", data_size);
        }
        Err(e) => {
            eprintln!("数据库缓存写入错误: {}", e);
        }
    }
}
