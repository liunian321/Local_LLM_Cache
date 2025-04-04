use crate::models::api_model::{
    AppState, ChatChoice, ChatMessageJson, ChatRequestJson, ChatResponseJson, Usage,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use brotli::CompressorWriter;
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::env;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;
use uuid::Uuid;

type TaskSender = tokio::sync::mpsc::Sender<BoxFuture<'static, ()>>;

// 修改 chat_completion 函数签名以适应新的状态类型
#[axum::debug_handler]
pub async fn chat_completion(
    State(app_state): State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
    Json(payload): Json<ChatRequestJson>,
) -> impl IntoResponse {
    let (state, tx_miss, tx_hit) = &*app_state;

    // 从环境变量中读取缓存版本和覆盖模式
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

    // 从请求中提取用户消息
    let user_message = match payload
        .messages
        .iter()
        .find(|msg| msg.role.to_lowercase() == "user")
    {
        Some(msg) => msg,
        None => {
            return (StatusCode::BAD_REQUEST, "未找到用户消息".to_string()).into_response();
        }
    };

    // 计算缓存键
    let mut hasher = Sha256::new();
    hasher.update(user_message.content.as_bytes());
    let hash = hex::encode(hasher.finalize());
    let cache_key = hash;

    // 创建响应通道
    let (tx, rx) = oneshot::channel();

    // 查询缓存并分发到适当的处理器
    let db_arc = state.db.clone();
    let key = cache_key.clone();

    // 克隆需要在任务中使用的变量
    let api_url = state.api_url.clone();
    let client = state.client.clone();
    let payload_clone = payload.clone();
    let tx_miss = tx_miss.clone();
    let tx_hit = tx_hit.clone();

    // 立即将查询和分发逻辑推送到任务队列
    tokio::spawn(async move {
        // 查询缓存
        let cached_result = if cache_override_mode {
            sqlx::query_as::<_, (Vec<u8>,)>(
                "SELECT response FROM cache WHERE key = ? AND version = ?",
            )
            .bind(key.clone())
            .bind(cache_version)
            .fetch_optional(&*db_arc)
            .await
        } else {
            sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ?")
                .bind(key.clone())
                .fetch_optional(&*db_arc)
                .await
        };

        // 处理查询结果
        let cached: Option<Vec<u8>> = match cached_result {
            Ok(data) => data.map(|(bytes,)| bytes),
            Err(e) => {
                let _ = tx.send(Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("数据库查询错误: {}", e),
                )));
                return;
            }
        };

        if let Some(compressed_response) = cached {
            // 缓存命中，发送到 hit 处理线程
            let (inner_tx, inner_rx) = oneshot::channel();

            let task: BoxFuture<'static, ()> = Box::pin(async move {
                let hit_start_time = Instant::now();

                // 解压缩处理逻辑
                let mut decompressed = Vec::new();
                let mut decompressor = brotli::Decompressor::new(
                    compressed_response.as_slice(),
                    compressed_response.len(),
                );

                let result = match std::io::copy(&mut decompressor, &mut decompressed) {
                    Ok(_) => {
                        match String::from_utf8(decompressed) {
                            Ok(message_content) => {
                                // 更新缓存命中计数
                                if let Err(e) = sqlx::query(
                                    "UPDATE cache SET hit_count = hit_count + 1 WHERE key = ?",
                                )
                                .bind(key.clone())
                                .execute(&*db_arc)
                                .await
                                {
                                    eprintln!("更新缓存命中次数失败: {}", e);
                                }

                                // 构建响应
                                let response = ChatResponseJson {
                                    id: format!("cache-hit-{}", Uuid::new_v4()),
                                    object: "chat.completion".to_string(),
                                    created: chrono::Utc::now().timestamp(),
                                    model: payload_clone.model.clone(),
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

                                let duration = hit_start_time.elapsed();
                                println!("缓存命中处理耗时: {:?}", duration);

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

                // 发送结果
                let _ = inner_tx.send(result);
            });

            // 将任务发送到命中处理通道
            if let Err(e) = tx_hit.send(task).await {
                eprintln!("发送到命中处理线程失败: {}", e);
                let _ = tx.send(Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "处理缓存命中请求失败".to_string(),
                )));
                return;
            }

            // 等待内部任务完成
            match inner_rx.await {
                Ok(result) => {
                    let _ = tx.send(result);
                }
                Err(_) => {
                    let _ = tx.send(Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "处理缓存命中请求失败".to_string(),
                    )));
                }
            }
        } else {
            // 缓存未命中，发送到 miss 处理线程
            let (inner_tx, inner_rx) = oneshot::channel();

            let task: BoxFuture<'static, ()> = Box::pin(async move {
                // 这里实现未命中的原有逻辑
                let miss_start_time = Instant::now();
                let target_url = format!("{}/v1/chat/completions", api_url);

                let result = match process_cache_miss(
                    &client,
                    &target_url,
                    &payload_clone,
                    key.clone(),
                    &db_arc,
                    cache_version,
                )
                .await
                {
                    Ok(response) => {
                        let duration = miss_start_time.elapsed();
                        println!("缓存未命中处理耗时: {:?}", duration);
                        Ok(Json(response))
                    }
                    Err(e) => Err(e),
                };

                // 发送结果
                let _ = inner_tx.send(result);
            });

            // 将任务发送到未命中处理通道
            if let Err(e) = tx_miss.send(task).await {
                eprintln!("发送到未命中处理线程失败: {}", e);
                let _ = tx.send(Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "处理缓存未命中请求失败".to_string(),
                )));
                return;
            }

            // 等待内部任务完成
            match inner_rx.await {
                Ok(result) => {
                    let _ = tx.send(result);
                }
                Err(_) => {
                    let _ = tx.send(Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "处理缓存未命中请求失败".to_string(),
                    )));
                }
            }
        }
    });

    // 等待处理完成并返回结果
    match rx.await {
        Ok(result) => result.into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "处理请求过程中出错".to_string(),
        )
            .into_response(),
    }
}

// 未命中处理逻辑函数
async fn process_cache_miss(
    client: &reqwest::Client,
    target_url: &str,
    payload: &ChatRequestJson,
    cache_key: String,
    db: &sqlx::SqlitePool,
    cache_version: i32,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    // 序列化请求负载
    let payload_json = serde_json::to_string(payload).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("序列化请求负载失败: {}", e),
        )
    })?;

    // 构建 reqwest 请求
    let mut request_builder = client
        .post(target_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "llm_api_rust_client/1.0");

    // 如果 API URL 包含主机名，则设置 Host 头
    if let Ok(url_parsed) = reqwest::Url::parse(target_url) {
        if let Some(host) = url_parsed.host_str() {
            request_builder = request_builder.header("Host", host);
        }
    }

    let request_result = request_builder.body(payload_json.clone()).send().await;

    // 处理请求结果
    let upstream_response_result: Result<ChatResponseJson, (StatusCode, String)> =
        match request_result {
            Ok(res) => {
                // 检查上游 API 的响应状态码
                let status = res.status();
                if !status.is_success() {
                    let error_body = res
                        .text()
                        .await
                        .unwrap_or_else(|_| "无法读取错误响应体".to_string());
                    eprintln!("上游API错误响应体 ({}): {}", status, error_body);
                    return Err((
                        StatusCode::from_u16(status.as_u16())
                            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                        format!("上游API返回错误: 状态码 = {}", status),
                    ));
                }

                // 读取响应体文本
                let response_text = match res.text().await {
                    Ok(text) => text,
                    Err(e) => {
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("读取上游响应失败: {}", e),
                        ));
                    }
                };

                // 尝试反序列化响应体
                match serde_json::from_str::<ChatResponseJson>(&response_text) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        eprintln!("反序列化上游响应失败: {}, Body: {}", e, response_text);
                        Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "处理上游响应失败".to_string(),
                        ))
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
                    // 调用 curl 函数，它返回 Result<ChatResponseJson, ...>
                    send_request_with_curl(target_url, &payload_json).await
                } else {
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("请求上游API失败 (reqwest): {}", e),
                    ))
                }
            }
        };

    // 处理上游响应结果
    match upstream_response_result {
        Ok(response_json) => {
            // 检查 choices 和 message content
            if response_json.choices.is_empty() {
                eprintln!("上游 API 返回的 choices 数组为空");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "上游响应无效 (choices)".to_string(),
                ));
            }
            let message_content = &response_json.choices[0].message.content;
            if message_content.is_empty() {
                eprintln!("上游 API 返回的 message 内容为空");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "上游响应无效 (content)".to_string(),
                ));
            }

            // 压缩消息内容
            let message_bytes = message_content.as_bytes();
            let mut compressed = Vec::new();
            {
                let mut compressor = CompressorWriter::new(&mut compressed, 4096, 11, 22);
                if let Err(e) = compressor.write_all(message_bytes) {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("压缩响应失败: {}", e),
                    ));
                }
                if let Err(e) = compressor.flush() {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("刷新压缩器失败: {}", e),
                    ));
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
            .execute(db)
            .await;

            match insert_result {
                Ok(_) => {
                    println!("成功缓存响应 Size: {}", data_size);
                }
                Err(e) => {
                    // 数据库插入失败，仅记录错误，但仍然返回成功获取的响应
                    eprintln!("数据库缓存写入错误: {}", e);
                }
            }

            // 返回成功的响应
            Ok(response_json)
        }
        Err(e) => Err(e), // 直接传递上游请求或 curl 的错误
    }
}

// 使用 curl 发送请求的函数
async fn send_request_with_curl(
    url: &str,
    payload: &str,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    println!("使用curl发送请求到: {}", url);

    let curl_output = match tokio::process::Command::new("curl")
        .arg("-sS") // 静默模式，但显示错误
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-H")
        .arg("Accept: application/json")
        .arg("-d")
        .arg(payload)
        .arg(url)
        .output()
        .await
    {
        Ok(output) => output,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("执行curl命令失败: {}", e),
            ));
        }
    };

    if !curl_output.status.success() {
        let stderr = String::from_utf8_lossy(&curl_output.stderr);
        let stdout = String::from_utf8_lossy(&curl_output.stdout);
        eprintln!("curl命令执行失败: Stderr: {}, Stdout: {}", stderr, stdout);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "curl命令执行失败 (ExitCode={:?})",
                curl_output.status.code()
            ),
        ));
    }

    let response_text = String::from_utf8_lossy(&curl_output.stdout);

    // 解析为 ChatResponseJson
    match serde_json::from_str::<ChatResponseJson>(&response_text) {
        Ok(response_json) => Ok(response_json),
        Err(e) => {
            eprintln!("解析curl响应失败: {}, Body: {}", e, response_text);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "解析curl响应失败".to_string(),
            ))
        }
    }
}

// 处理 /v1/models 路由的请求
pub async fn get_models(
    State(state): State<Arc<AppState>>,
) -> Result<String, (StatusCode, String)> {
    let res = state
        .client
        .get(format!("{}/v1/models", state.api_url))
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response_text = res
        .text()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(response_text)
}

// 处理 /v1/embeddings 路由的请求
pub async fn get_embeddings(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Result<String, (StatusCode, String)> {
    let res = state
        .client
        .post(format!("{}/v1/embeddings", state.api_url))
        .json(&payload)
        .send()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response_text = res
        .text()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(response_text)
}
