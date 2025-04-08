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
use uuid::Uuid;

pub type TaskSender = tokio::sync::mpsc::Sender<BoxFuture<'static, ()>>;

// chat_completion
#[axum::debug_handler]
pub async fn chat_completion(
    State(app_state): State<Arc<(Arc<AppState>, TaskSender)>>,
    Json(payload): Json<ChatRequestJson>,
) -> Response {
    let (state, tx_hit) = {
        let (state_ref, tx_hit_ref) = &*app_state;
        (state_ref.clone(), tx_hit_ref.clone())
    };

    // 创建响应通道（ tokio::sync::oneshot 的无锁版本）
    let (response_tx, response_rx) = tokio::sync::oneshot::channel();

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

    // 读取缓存
    let cached_result = if cache_override_mode {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ? AND version = ?")
            .bind(cache_key.clone())
            .bind(cache_version)
            .fetch_optional(&*state.db)
            .await
    } else {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ?")
            .bind(cache_key.clone())
            .fetch_optional(&*state.db)
            .await
    };

    match cached_result {
        Ok(Some((compressed_response,))) => {
            // 缓存命中
            let hit_task = Box::pin(async move {
                // 解压缩处理
                let mut decompressed = Vec::new();
                let mut decompressor = brotli::Decompressor::new(
                    compressed_response.as_slice(),
                    compressed_response.len(),
                );

                let result = match std::io::copy(&mut decompressor, &mut decompressed) {
                    Ok(_) => {
                        match String::from_utf8(decompressed) {
                            Ok(message_content) => {
                                // 构建响应
                                let response = ChatResponseJson {
                                    id: format!("cache-hit-{}", Uuid::new_v4()),
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

                                Ok(Json(response))
                            }
                            Err(e) => Err((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("解析缓存内容失败: {}", e),
                            )),
                        }
                    }
                    Err(e) => Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("解压缩缓存数据失败: {}", e),
                    )),
                };

                if response_tx.send(result).is_err() {
                    // 如果发送失败，记录错误（可选）
                    eprintln!("无法发送响应结果");
                }
            });
            tx_hit.send(hit_task).await.unwrap();
        }
        Ok(None) => {
            // 缓存未命中逻辑
            // 克隆需要传递给异步任务的变量
            let target_url = format!("{}/v1/chat/completions", state.api_url);
            let client_clone = state.client.clone();
            let payload_clone = payload.clone();
            let cache_key_clone = cache_key.clone();
            let db_clone = state.db.clone();
            
            // 立即请求上游 API 并返回结果
            tokio::spawn(async move {
                // 构建请求
                let payload_json = match serde_json::to_string(&payload_clone) {
                    Ok(json) => json,
                    Err(e) => {
                        let _ = response_tx.send(Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("序列化请求负载失败: {}", e),
                        )));
                        return;
                    }
                };

                // 构建 reqwest 请求
                let mut request_builder = client_clone
                    .post(&target_url)
                    .header("Content-Type", "application/json")
                    .header("Accept", "application/json")
                    .header("User-Agent", "llm_api_rust_client/1.0");

                // 如果 API URL 包含主机名，则设置 Host 头
                if let Ok(url_parsed) = reqwest::Url::parse(&target_url) {
                    if let Some(host) = url_parsed.host_str() {
                        request_builder = request_builder.header("Host", host);
                    }
                }

                // 发送请求
                let request_result = request_builder.body(payload_json.clone()).send().await;

                // 处理请求结果
                let response_result = match request_result {
                    Ok(res) => {
                        // 检查上游 API 的响应状态码
                        let status = res.status();
                        if !status.is_success() {
                            let error_body = res
                                .text()
                                .await
                                .unwrap_or_else(|_| "无法读取错误响应体".to_string());
                            eprintln!("上游API错误响应体 ({}): {}", status, error_body);
                            Err((
                                StatusCode::from_u16(status.as_u16())
                                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                                format!("上游API返回错误: 状态码 = {}", status),
                            ))
                        } else {
                            // 读取响应体文本
                            match res.text().await {
                                Ok(text) => {
                                    // 反序列化响应体
                                    match serde_json::from_str::<ChatResponseJson>(&text) {
                                        Ok(json) => Ok(json),
                                        Err(e) => {
                                            eprintln!("反序列化上游响应失败: {}, Body: {}", e, text);
                                            Err((
                                                StatusCode::INTERNAL_SERVER_ERROR,
                                                "处理上游响应失败".to_string(),
                                            ))
                                        }
                                    }
                                }
                                Err(e) => {
                                    Err((
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        format!("读取上游响应失败: {}", e),
                                    ))
                                }
                            }
                        }
                    }
                    Err(e) => {
                        // reqwest 请求失败，尝试 curl
                        eprintln!("使用 reqwest 客户端请求失败: {}, 尝试使用 curl 作为备选", e);
                        let use_curl: bool = env::var("USE_CURL")
                            .unwrap_or_else(|_| "false".to_string())
                            .parse::<bool>()
                            .unwrap_or(false);
                        if use_curl {
                            // 调用 curl 函数
                            send_request_with_curl(&target_url, &payload_json).await
                        } else {
                            Err((
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("请求上游API失败 (reqwest): {}", e),
                            ))
                        }
                    }
                };

                // 根据处理结果创建新结果以发送给客户端
                let new_result = match &response_result {
                    Ok(json) => Ok(Json(json.clone())),
                    Err((status, message)) => Err((*status, message.clone())),
                };
                
                // 发送响应结果给等待的处理程序
                let _ = response_tx.send(new_result);
                
                // 如果响应成功，在后台进行缓存写入操作
                if let Ok(response_json) = response_result {
                    tokio::spawn(async move {
                        cache_response(response_json, cache_key_clone, db_clone, cache_version).await;
                    });
                }
            });
        }
        Err(e) => {
            let _ = response_tx.send(Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("数据库查询错误: {}", e),
            )));
        }
    }

    // 等待响应
    match response_rx.await {
        Ok(result) => result.into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "请求处理失败").into_response(),
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
