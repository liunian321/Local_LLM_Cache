use crate::handlers::api_handler::send_request_with_curl;
use crate::handlers::proxy_handler::send_proxied_request;
use crate::models::api_model::{
    AppState, ChatChoice, ChatMessageJson, ChatRequestJson, ChatResponseJson, Usage,
    select_api_endpoint,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use brotli::CompressorWriter;
use futures::future::BoxFuture;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;
use crate::utils::db_writer::DbWriter;

pub type TaskSender = tokio::sync::mpsc::Sender<BoxFuture<'static, ()>>;

// 缓存查询的异步函数
async fn query_cache(
    db: Arc<sqlx::SqlitePool>,
    question_key: String,
    cache_version: u8,
    cache_override_mode: bool,
    memory_cache: Option<&Arc<crate::utils::memory_cache::MemoryCache>>,
    cache_enabled: bool,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    // 如果内存缓存已禁用，直接查询数据库
    if !cache_enabled {
        return query_db_cache(db, question_key, cache_version, cache_override_mode).await;
    }
    
    // 如果启用了内存缓存，先从内存中查找
    if let Some(cache) = memory_cache {
        if let Some(data) = cache.get(&question_key) {
            println!("内存缓存命中");
            return Ok(Some(data));
        }
    }
    
    // 内存缓存未命中，查询数据库
    println!("内存缓存未命中，查询数据库");
    query_db_cache(db, question_key, cache_version, cache_override_mode).await
}

// 数据库缓存查询函数
async fn query_db_cache(
    db: Arc<sqlx::SqlitePool>,
    question_key: String,
    cache_version: u8,
    cache_override_mode: bool,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    println!("并行查询缓存");

    // 修改查询语句，同时获取 response 和 answer_key
    let result = if cache_override_mode {
        sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT a.response, a.key 
             FROM questions q 
             JOIN answers a ON q.answer_key = a.key 
             WHERE q.key = ? AND a.version >= ?
             LIMIT 1",
        )
        .bind(question_key.clone())
        .bind(cache_version)
        .fetch_optional(&*db)
        .await?
    } else {
        sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT a.response, a.key 
             FROM questions q 
             JOIN answers a ON q.answer_key = a.key 
             WHERE q.key = ?
             LIMIT 1",
        )
        .bind(question_key.clone())
        .fetch_optional(&*db)
        .await?
    };

    // 如果找到缓存项，更新答案表中的命中计数
    if let Some((_, answer_key)) = &result {
        let db_clone = db.clone();
        let answer_key_clone = answer_key.clone();

        tokio::spawn(async move {
            // 更新命中次数
            match sqlx::query("UPDATE answers SET hit_count = hit_count + 1 WHERE key = ?")
                .bind(answer_key_clone)
                .execute(&*db_clone)
                .await
            {
                Ok(_) => (),
                Err(e) => {
                    println!("更新缓存命中计数失败: {}", e);
                }
            }
        });
    }

    Ok(result.map(|(data, _)| data))
}

// 处理解压缩缓存内容
async fn process_cached_response(
    compressed_data: Vec<u8>,
    payload: ChatRequestJson,
) -> Result<Json<ChatResponseJson>, (StatusCode, String)> {
    let mut decompressed = Vec::new();
    let mut decompressor =
        brotli::Decompressor::new(compressed_data.as_slice(), compressed_data.len());

    match std::io::copy(&mut decompressor, &mut decompressed) {
        Ok(_) => match String::from_utf8(decompressed) {
            Ok(message_content) => {
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

// 发送API请求函数
async fn send_api_request(
    client: reqwest::Client,
    target_url: String,
    payload_json: String,
    permit: tokio::sync::OwnedSemaphorePermit,
    use_curl: bool,
    use_proxy: bool,
    headers: &std::collections::HashMap<String, String>,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    // 记录信号量使用
    let _permit = permit;
    let request_id = uuid::Uuid::new_v4()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();
    let start_time = Instant::now();

    // 根据配置选择请求方式
    if use_curl {
        println!("[{}] 使用curl模式发送请求", request_id);
        return send_request_with_curl(&target_url, &payload_json).await;
    } else if use_proxy {
        println!("[{}] 使用代理模式发送请求", request_id);
        let result = send_proxied_request(&target_url, &payload_json, headers).await;
        println!(
            "[{}] 代理请求已完成 ({:?})",
            request_id,
            start_time.elapsed()
        );
        return result;
    }

    // 创建请求构建器
    let mut request_builder = client.post(&target_url);

    // 添加请求头
    for (key, value) in headers {
        request_builder = request_builder.header(key, value);
    }

    if !headers.contains_key("Content-Type") {
        request_builder = request_builder.header("Content-Type", "application/json");
    }

    // 发送请求
    let response = match tokio::time::timeout(
        Duration::from_secs(60), // 增加超时时间
        request_builder.body(payload_json).send(),
    )
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => {
            println!("[{}] 请求失败: {}", request_id, e);
            if e.is_connect() {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("无法连接到上游服务器(连接错误): {}", e),
                ));
            } else if e.is_timeout() {
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("上游服务器响应超时: {}", e),
                ));
            } else {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("请求上游服务器失败: {}", e),
                ));
            }
        }
        Err(_) => {
            println!("[{}] 请求发送超时", request_id);
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "请求上游服务器超时".to_string(),
            ));
        }
    };

    // 检查状态码
    if !response.status().is_success() {
        return Err((
            StatusCode::from_u16(response.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            format!("上游服务器返回错误: {:?}", response),
        ));
    }

    let text = match tokio::time::timeout(
        Duration::from_secs(60), // 增加读取超时时间
        response.text(),
    )
    .await
    {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            println!("[{}] 读取响应体失败: {}", request_id, e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("读取响应体失败: {}", e),
            ));
        }
        Err(_) => {
            println!("[{}] 读取上游服务器响应超时", request_id);
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "读取上游服务器响应超时".to_string(),
            ));
        }
    };

    match serde_json::from_str::<ChatResponseJson>(&text) {
        Ok(json) => Ok(json),
        Err(e) => {
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(generic_json) => {
                    // 尝试提取必要的字段并构造 ChatResponseJson
                    let choices = match generic_json.get("choices") {
                        Some(choices) => {
                            if let Some(choices_array) = choices.as_array() {
                                choices_array
                                    .iter()
                                    .enumerate()
                                    .map(|(idx, choice)| {
                                        let content = match choice
                                            .get("message")
                                            .and_then(|m| m.get("content"))
                                        {
                                            Some(content) => {
                                                content.as_str().unwrap_or("").to_string()
                                            }
                                            None => "".to_string(),
                                        };

                                        let role =
                                            match choice.get("message").and_then(|m| m.get("role"))
                                            {
                                                Some(role) => {
                                                    role.as_str().unwrap_or("assistant").to_string()
                                                }
                                                None => "assistant".to_string(),
                                            };

                                        let finish_reason = match choice.get("finish_reason") {
                                            Some(reason) => {
                                                reason.as_str().unwrap_or("unknown").to_string()
                                            }
                                            None => "unknown".to_string(),
                                        };

                                        ChatChoice {
                                            index: idx as i32,
                                            logprobs: None,
                                            finish_reason,
                                            message: ChatMessageJson { role, content },
                                        }
                                    })
                                    .collect()
                            } else {
                                vec![]
                            }
                        }
                        None => vec![],
                    };

                    if choices.is_empty() {
                        println!("[{}] 无法从通用JSON中提取有效的消息内容", request_id);
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("解析响应JSON失败: {}", e),
                        ));
                    }

                    let response = ChatResponseJson {
                        id: generic_json
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        object: generic_json
                            .get("object")
                            .and_then(|v| v.as_str())
                            .unwrap_or("chat.completion")
                            .to_string(),
                        created: generic_json
                            .get("created")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(chrono::Utc::now().timestamp()),
                        model: generic_json
                            .get("model")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        choices,
                        usage: Usage {
                            prompt_tokens: generic_json
                                .get("usage")
                                .and_then(|u| u.get("prompt_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0) as i32,
                            completion_tokens: generic_json
                                .get("usage")
                                .and_then(|u| u.get("completion_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0) as i32,
                            total_tokens: generic_json
                                .get("usage")
                                .and_then(|u| u.get("total_tokens"))
                                .and_then(|v| v.as_i64())
                                .unwrap_or(0) as i32,
                        },
                        stats: serde_json::Value::Null,
                        system_fingerprint: generic_json
                            .get("system_fingerprint")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                    };

                    println!("[{}] 成功构造兼容的响应对象", request_id);
                    Ok(response)
                }
                Err(parse_err) => {
                    println!("[{}] 解析为通用JSON也失败: {}", request_id, parse_err);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("解析响应JSON失败: {}", e),
                    ))
                }
            }
        }
    }
}

// chat_completion
#[axum::debug_handler]
pub async fn chat_completion(
    State(app_state): State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ChatRequestJson>,
) -> Response {
    let request_id = uuid::Uuid::new_v4()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();

    let (state, _tx_hit, _tx_miss) = {
        let (state_ref, tx_hit_ref, tx_miss_ref) = &*app_state;
        (state_ref.clone(), tx_hit_ref.clone(), tx_miss_ref.clone())
    };

    // 提取用户消息并计算问题的哈希作为键
    let user_message = match payload
        .messages
        .iter()
        .find(|msg| msg.role.to_lowercase() == "user")
    {
        Some(msg) => msg,
        None => {
            println!("[{}] 错误: 未找到用户消息", request_id);
            return (StatusCode::BAD_REQUEST, "未找到用户消息").into_response();
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(user_message.content.as_bytes());
    let question_key = hex::encode(hasher.finalize());

    // 选择API端点
    let selected_endpoint = if !state.api_endpoints.is_empty() {
        match select_api_endpoint(&state.api_endpoints) {
            Some(endpoint) => endpoint,
            None => {
                println!("[{}] 错误: 没有可用的API端点", request_id);
                return (StatusCode::SERVICE_UNAVAILABLE, "没有可用的 API 端点").into_response();
            }
        }
    } else {
        println!("[{}] 错误: API端点列表为空", request_id);
        return (StatusCode::SERVICE_UNAVAILABLE, "没有配置 API 端点").into_response();
    };

    // 如果是流式请求，跳过缓存
    let skip_cache = payload.stream;

    // 查询缓存（除非是流式请求）
    let cache_result = if skip_cache {
        Ok(None)
    } else {
        query_cache(
            state.db.clone(),
            question_key.clone(),
            selected_endpoint.version,
            state.cache_override_mode,
            state.memory_cache.as_ref(),
            state.cache_enabled,
        )
        .await
    };

    match cache_result {
        Ok(Some(compressed_data)) => {
            println!("[{}] 缓存命中", request_id);
            match process_cached_response(compressed_data, payload).await {
                Ok(json) => {
                    println!("[{}] 成功处理缓存响应", request_id);
                    json.into_response()
                }
                Err((status, message)) => {
                    println!(
                        "[{}] 处理缓存响应错误: {} - {}",
                        request_id, status, message
                    );
                    (status, message).into_response()
                }
            }
        }
        Ok(None) => {
            println!("[{}] 缓存未命中. 将进行API请求", request_id);

            // 获取信号量
            println!(
                "[{}] 尝试获取信号量许可... (当前可用: {})",
                request_id,
                state.semaphore.available_permits()
            );

            // 设置获取信号量的超时
            let permit = match tokio::time::timeout(
                Duration::from_secs(10), // 10秒超时
                state.semaphore.clone().acquire_owned(),
            )
            .await
            {
                Ok(Ok(p)) => {
                    println!(
                        "[{}] 成功获取信号量许可 (剩余: {})",
                        request_id,
                        state.semaphore.available_permits()
                    );
                    p
                }
                Ok(Err(e)) => {
                    println!("[{}] 获取信号量许可失败: {}", request_id, e);
                    return (StatusCode::INTERNAL_SERVER_ERROR, "获取并发许可失败").into_response();
                }
                Err(_) => {
                    println!("[{}] 获取信号量许可超时", request_id);
                    return (StatusCode::SERVICE_UNAVAILABLE, "服务器忙，请稍后再试")
                        .into_response();
                }
            };

            let target_url = if selected_endpoint.url.ends_with('/') {
                format!("{}v1/chat/completions", selected_endpoint.url)
            } else {
                format!("{}/v1/chat/completions", selected_endpoint.url)
            };

            // 创建请求载荷的副本
            let mut payload_clone = payload.clone();

            // 如果端点配置了model，则使用端点配置的model
            if let Some(model) = selected_endpoint.model {
                payload_clone.model = model;
            }
            
            // 如果配置了思考参数，则设置enable_thinking参数
            if state.enable_thinking.is_some() {
                payload_clone.enable_thinking = state.enable_thinking;
            }

            // 序列化请求负载
            let payload_json = match serde_json::to_string(&payload_clone) {
                Ok(json) => json,
                Err(e) => {
                    println!("[{}] 序列化请求负载失败: {}", request_id, e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("序列化请求负载失败: {}", e),
                    )
                        .into_response();
                }
            };

            // 提取客户端请求头并转换为HashMap
            let mut client_headers = std::collections::HashMap::new();
            for (key, value) in headers.iter() {
                if let Ok(v) = value.to_str() {
                    // 过滤掉一些可能会干扰请求的头
                    let key_lower = key.as_str().to_lowercase();
                    if !key_lower.contains("connection")
                        && !key_lower.contains("host")
                        && !key_lower.contains("content-length")
                    {
                        client_headers.insert(key.as_str().to_string(), v.to_string());
                    }
                }
            }

            // 添加API配置中的自定义头
            for (key, value) in &state.api_headers {
                client_headers.insert(key.clone(), value.clone());
            }

            let api_result = send_api_request(
                state.client.clone(),
                target_url,
                payload_json,
                permit,
                state.use_curl,
                state.use_proxy,
                &client_headers,
            )
            .await;

            match &api_result {
                Ok(response_json) => {
                    let response_clone = response_json.clone();
                    let db_clone = state.db.clone();

                    // 在后台执行缓存操作（如果不是流式请求）
                    if !skip_cache {
                        tokio::spawn(async move {
                            cache_response(
                                response_clone,
                                question_key,
                                db_clone,
                                selected_endpoint.version,
                                state.memory_cache.clone(),
                                state.cache_enabled,
                                state.batch_write_size,
                            )
                            .await;
                        });
                    }

                    Json(response_json.clone()).into_response()
                }
                Err((status, msg)) => (status.clone(), msg.clone()).into_response(),
            }
        }
        Err(e) => {
            // 数据库查询错误
            println!("[{}] 数据库查询错误: {}", request_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("数据库查询错误: {}", e),
            )
                .into_response()
        }
    }
}

// 缓存响应函数
async fn cache_response(
    response_json: ChatResponseJson,
    question_key: String,
    db: Arc<sqlx::SqlitePool>,
    cache_version: u8,
    memory_cache: Option<Arc<crate::utils::memory_cache::MemoryCache>>,
    cache_enabled: bool,
    batch_write_size: usize,
) {
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
    let mut compressed = Vec::with_capacity(message_bytes.len() / 2); // 预分配大小
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
    let cache_max_size = 5 * 1024 * 1024; // 5MB

    // 如果压缩后大小超过限制，跳过缓存
    if data_size > cache_max_size {
        eprintln!(
            "响应体积过大 ({} bytes)，超过缓存限制 ({} bytes)，跳过缓存",
            data_size, cache_max_size
        );
        return;
    }

    // 计算响应内容的哈希作为答案的 key
    let mut hasher = Sha256::new();
    hasher.update(message_bytes);

    // 如果启用了内存缓存，先添加到内存缓存
    if cache_enabled {
        if let Some(cache) = memory_cache {
            // 将响应添加到内存缓存
            tokio::spawn(async move {
                cache.insert(question_key, compressed.clone()).await;
                
                // 如果待写入队列达到了批量写入阈值，执行批量写入
                if cache.pending_count() >= batch_write_size {
                    println!("内存缓存待写入队列达到阈值 ({})，执行批量写入", batch_write_size);
                    let pending_items = cache.take_pending_writes(batch_write_size);
                    
                    // 创建数据库写入工具并执行批量写入
                    let db_writer = DbWriter::new(db, cache_version);
                    let (success, failed) = db_writer.batch_write(pending_items).await;
                    println!("批量写入完成，成功: {}，失败: {}", success, failed);
                }
            });
            return; // 已经添加到内存缓存，不需要继续执行
        }
    }

    // 如果没有启用内存缓存，或内存缓存创建失败，直接写入数据库
    let db_writer = DbWriter::new(db, cache_version);
    if db_writer.write_single(question_key, compressed).await {
        println!("成功写入单个响应到数据库");
    } else {
        eprintln!("写入单个响应到数据库失败");
    }
}
