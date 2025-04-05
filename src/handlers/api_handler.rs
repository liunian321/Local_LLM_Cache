use crate::models::api_model::{
    AppState, ChatChoice, ChatMessageJson, ChatRequestJson, ChatResponseJson, Usage,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
};
use brotli::CompressorWriter;
use sha2::{Digest, Sha256};
use std::env;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

#[axum::debug_handler]
pub async fn chat_completion(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequestJson>,
) -> Result<Json<ChatResponseJson>, (StatusCode, String)> {
    // 记录请求开始时间
    let start_time: Instant = Instant::now();

    // 从环境变量中读取缓存版本和覆盖模式
    let cache_version: i32 = env::var("CACHE_VERSION")
        .unwrap_or_else(|_| "0".to_string())
        .parse()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("解析缓存版本失败: {}", e),
            )
        })?;
    let cache_override_mode: bool = env::var("CACHE_OVERRIDE_MODE")
        .unwrap_or_else(|_| "false".to_string())
        .parse()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("解析覆盖模式失败: {}", e),
            )
        })?;

    // 从请求中提取用户消息（role 为 "user"）
    let user_message = payload
        .messages
        .iter()
        .find(|msg| msg.role.to_lowercase() == "user")
        .ok_or((StatusCode::BAD_REQUEST, "未找到用户消息".to_string()))?;

    // 对用户消息内容进行 SHA256 哈希
    let mut hasher = Sha256::new();
    hasher.update(user_message.content.as_bytes());
    let hash = hex::encode(hasher.finalize());
    // 组合 model 与 hash 作为缓存 key
    let cache_key = hash;

    // 查询数据库中是否存在缓存
    let db_arc = state.db.clone(); // 克隆 Arc
    let key = cache_key.clone();
    let cached_result = if cache_override_mode {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ? AND version = ?")
            .bind(key)
            .bind(cache_version)
            .fetch_optional(&*db_arc)
            .await
    } else {
        sqlx::query_as::<_, (Vec<u8>,)>("SELECT response FROM cache WHERE key = ?")
            .bind(key)
            .fetch_optional(&*db_arc)
            .await
    };

    // 处理数据库查询结果
    let cached: Option<Vec<u8>> = match cached_result {
        Ok(data) => data.map(|(bytes,)| bytes),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("数据库查询错误: {}", e),
            ));
        }
    };

    if let Some(compressed_response) = cached {
        // 缓存命中，使用 hit_runtime 异步处理
        let hit_runtime = state.hit_runtime.clone();
        let db_arc = state.db.clone();
        let key = cache_key.clone();
        let payload_model = payload.model.clone();
        
        let join_handle = hit_runtime.spawn(async move {
            let hit_start_time = Instant::now();
            
            // 使用 brotli 解压缩数据
            let mut decompressed = Vec::new();
            let mut decompressor =
                brotli::Decompressor::new(compressed_response.as_slice(), compressed_response.len());
            match std::io::copy(&mut decompressor, &mut decompressed) {
                Ok(_) => {},
                Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
            };

            // 将解压缩后的数据反序列化为字符串
            let message_content = match String::from_utf8(decompressed) {
                Ok(content) => content,
                Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
            };

            // 异步更新缓存命中次数
            if let Err(e) = sqlx::query("UPDATE cache SET hit_count = hit_count + 1 WHERE key = ?")
                .bind(key.clone())
                .execute(&*db_arc)
                .await {
                eprintln!("更新缓存命中次数失败: {}", e);
            }

            // 组装响应结果
            let response = ChatResponseJson {
                id: format!("cache-hit-{}", Uuid::new_v4()),
                object: "chat.completion".to_string(),
                created: chrono::Utc::now().timestamp(),
                model: payload_model,
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

            // 打印缓存命中耗时
            let duration = hit_start_time.elapsed();
            println!("缓存命中处理耗时: {:?}", duration);

            Ok(response)
        });

        // 等待 hit_runtime 上的任务完成并处理结果
        match join_handle.await {
            Ok(Ok(response_json)) => {
                let total_duration = start_time.elapsed();
                println!("缓存命中总耗时: {:?}", total_duration);
                return Ok(Json(response_json));
            },
            Ok(Err(err)) => return Err(err),
            Err(e) => {
                eprintln!(
                    "处理缓存命中任务失败 (JoinError): {}, Key: {}",
                    e, cache_key
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("处理缓存命中请求失败 (JoinError), Key={}", cache_key),
                ));
            }
        }
    } else {
        // 缓存未命中，调用上游 API
        let api_url = state.api_url.clone(); // 克隆 api_url
        let client = state.client.clone(); // 克隆 client
        let miss_runtime = state.miss_runtime.clone(); // 克隆 miss_runtime
        let cache_key_clone = cache_key.clone(); // 克隆 cache_key
        let join_handle = miss_runtime.spawn(async move {
            let miss_start_time = Instant::now();
            let target_url = format!("{}/v1/chat/completions", api_url); // 使用克隆的 api_url
            let payload_json = serde_json::to_string(&payload).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("序列化请求负载失败: {}", e),
                )
            })?;

            // 构建 reqwest 请求 (移除了手动构建，使用更简洁的方式)
            let mut request_builder = client.post(&target_url)
                .header("Content-Type", "application/json")
                .header("Accept", "application/json")
                .header("User-Agent", "llm_api_rust_client/1.0");

             // 如果 API URL 包含主机名，则设置 Host 头
             if let Ok(url_parsed) = reqwest::Url::parse(&target_url) {
                 if let Some(host) = url_parsed.host_str() {
                     request_builder = request_builder.header("Host", host);
                 }
             }

            let request_result = request_builder
                .body(payload_json.clone()) // clone payload_json 给 reqwest
                .send()
                .await;


            // 处理 reqwest 或 curl 的结果
            let upstream_response_result: Result<ChatResponseJson, (StatusCode, String)> = match request_result {
                Ok(res) => {
                    // 检查上游 API 的响应状态码
                    let status = res.status();
                    if !status.is_success() {
                        let error_body = res.text().await.unwrap_or_else(|_| "无法读取错误响应体".to_string());
                        eprintln!("上游API错误响应体 ({}): {}", status, error_body);
                        return Err((
                            StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                            format!("上游API返回错误: 状态码 = {}", status), // 隐藏具体错误信息给客户端
                        ));
                    }

                    // 读取响应体文本
                    let response_text = match res.text().await {
                        Ok(text) => text,
                        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("读取上游响应失败: {}", e))),
                    };

                    // 尝试反序列化响应体
                    match serde_json::from_str::<ChatResponseJson>(&response_text) {
                        Ok(json) => Ok(json),
                        Err(e) => {
                             eprintln!("反序列化上游响应失败: {}, Body: {}", e, response_text);
                             Err((StatusCode::INTERNAL_SERVER_ERROR, "处理上游响应失败".to_string()))
                        }
                    }
                },
                Err(e) => {
                    // reqwest 请求失败，尝试 curl
                    eprintln!("使用 reqwest 客户端请求失败: {}, 尝试使用 curl 作为备选", e);
                    let use_curl: bool = env::var("USE_CURL").unwrap_or_else(|_| "false".to_string()).parse::<bool>().unwrap_or(false);
                    if use_curl {
                        // 调用修改后的 curl 函数，它返回 Result<ChatResponseJson, ...>
                        send_request_with_curl(&target_url, &payload_json).await
                    } else {
                        Err((StatusCode::INTERNAL_SERVER_ERROR, format!("请求上游API失败 (reqwest): {}", e)))
                    }
                }
            };

            // 无论 reqwest 还是 curl 成功，都尝试处理和缓存
            match upstream_response_result {
                Ok(response_json) => {
                     // 检查 choices 和 message content
                     if response_json.choices.is_empty() {
                         eprintln!("上游 API 返回的 choices 数组为空");
                         return Err((StatusCode::INTERNAL_SERVER_ERROR,"上游响应无效 (choices)".to_string()));
                     }
                     let message_content = &response_json.choices[0].message.content;
                     if message_content.is_empty() {
                         eprintln!("上游 API 返回的 message 内容为空");
                         return Err((StatusCode::INTERNAL_SERVER_ERROR,"上游响应无效 (content)".to_string()));
                     }

                     // 压缩消息内容
                     let message_bytes = message_content.as_bytes();
                     let mut compressed = Vec::new();
                     { // 使用块来限制 compressor 的生命周期，确保 flush
                         let mut compressor = CompressorWriter::new(&mut compressed, 4096, 11, 22);
                         if let Err(e) = compressor.write_all(message_bytes) {
                             return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("压缩响应失败: {}", e)));
                         }
                         if let Err(e) = compressor.flush() {
                             return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("刷新压缩器失败: {}", e)));
                         }
                     } // compressor 在这里 drop，内部 writer (&mut compressed) 的引用释放

                    let data_size = compressed.len() as i64;
                    let db_arc_insert = state.db.clone();
                    let key_for_insert = cache_key_clone.clone(); // 再次 clone key
                    let data_to_insert = compressed; // 移动所有权
                    
                    let insert_result = sqlx::query(
                        "INSERT OR REPLACE INTO cache (key, response, size, hit_count, version) VALUES (?, ?, ?, 0, ?)"
                    )
                    .bind(key_for_insert)
                    .bind(data_to_insert)
                    .bind(data_size)
                    .bind(cache_version)
                    .execute(&*db_arc_insert)
                    .await;

                    match insert_result {
                        Ok(_) => {
                             println!("成功缓存响应 Size: {}", data_size);
                        },
                        Err(e) => {
                            // 数据库插入失败，仅记录错误，但仍然返回成功获取的响应
                            eprintln!("数据库缓存写入错误: {}", e);
                        }
                    }

                    // 打印正常请求第三方接口耗时
                    let duration = miss_start_time.elapsed();
                    println!("缓存未命中处理耗时: {:?}", duration);

                    // 返回成功的响应
                    Ok(response_json)
                }
                Err(e) => Err(e), // 直接传递上游请求或 curl 的错误
            }

        });

        // 等待 miss_runtime 上的任务完成并处理结果
        match join_handle.await {
            Ok(Ok(response_json)) => Ok(Json(response_json)), // 任务成功完成并返回 Ok(ChatResponseJson)
            Ok(Err(err)) => Err(err), // 任务成功完成但返回应用级错误 Err((StatusCode, String))
            Err(e) => {
                // 任务本身执行失败 (panic 或取消)
                eprintln!(
                    "处理缓存未命中任务失败 (JoinError): {}, Key: {}",
                    e, cache_key
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("处理请求失败 (JoinError), Key={}", cache_key),
                ));
            }
        }
    }
}

// 修改 curl 函数的返回类型
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

    // 解析为 ChatResponseJson，但不包装在 Json() 中
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
