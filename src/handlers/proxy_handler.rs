use crate::models::api_model::{ChatChoice, ChatMessageJson, ChatResponseJson, Usage};
use axum::http::StatusCode;
use std::error::Error as StdError;
use std::time::{Duration, Instant};

pub async fn send_proxied_request(
    client: reqwest::Client,
    target_url: &str,
    payload_json: &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    // 生成请求 ID，用于日志追踪
    let request_id = uuid::Uuid::new_v4()
        .to_string()
        .chars()
        .take(8)
        .collect::<String>();

    let start_time = Instant::now();

    // 使用完全克隆的新客户端以避免任何可能的共享状态
    let new_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .pool_max_idle_per_host(5) // 使用合理的连接池数量
        .no_proxy()
        .build()
        .unwrap_or_else(|e| {
            println!(
                "[{}] 警告: 无法创建新HTTP客户端: {}，使用原客户端",
                request_id, e
            );
            client.clone()
        });

    // 创建请求构建器
    let mut request_builder = new_client.post(target_url);

    // 设置请求头
    for (key, value) in headers {
        request_builder = request_builder.header(key, value);
    }

    if !headers.contains_key("Content-Type") {
        request_builder = request_builder.header("Content-Type", "application/json");
    }

    println!("[{}] 开始发送请求...", request_id);

    // 发送请求（设置超时）
    let response = match tokio::time::timeout(
        Duration::from_secs(30), // 增加到30秒超时
        request_builder.body(payload_json.to_owned()).send(),
    )
    .await
    {
        Ok(Ok(response)) => {
            println!(
                "[{}] 成功收到响应: 状态码 = {} ({:?})",
                request_id,
                response.status(),
                start_time.elapsed()
            );
            response
        }
        Ok(Err(e)) => {
            println!(
                "[{}] 请求发送失败: {} ({:?})",
                request_id,
                e,
                start_time.elapsed()
            );
            println!("[{}] 错误详情: {:?}", request_id, e);

            // 尝试检查错误源
            let mut source_err: Option<&dyn StdError> = Some(&e);
            while let Some(err) = source_err {
                println!("[{}] 错误链: {}", request_id, err);
                source_err = err.source();
            }

            // 详细的错误类型判断
            if e.is_connect() {
                println!("[{}] 错误类型: 连接错误", request_id);
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("无法连接到上游服务器(连接错误): {}", e),
                ));
            } else if e.is_timeout() {
                println!("[{}] 错误类型: 超时", request_id);
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("上游服务器响应超时: {}", e),
                ));
            } else if e.is_request() {
                println!("[{}] 错误类型: 请求构建错误", request_id);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("请求构建错误: {}", e),
                ));
            } else if e.is_redirect() {
                println!("[{}] 错误类型: 重定向错误", request_id);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("重定向错误: {}", e),
                ));
            } else {
                println!("[{}] 错误类型: 未知错误", request_id);
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("请求上游服务器失败: {}", e),
                ));
            }
        }
        Err(_) => {
            println!(
                "[{}] 请求发送超时 (30秒) ({:?})",
                request_id,
                start_time.elapsed()
            );
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "连接上游服务器超时".to_string(),
            ));
        }
    };

    // 检查响应状态
    if !response.status().is_success() {
        return Err((
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            format!(
                "上游服务器返回错误: {:?}",
                response
            ),
        ));
    }

    // 读取响应体
    println!("[{}] 状态码正常，开始读取响应体...", request_id);
    let text = match tokio::time::timeout(
        Duration::from_secs(30), // 30秒读取超时
        response.text(),
    )
    .await
    {
        Ok(Ok(text)) => {
            println!("[{}] 成功获取响应体，长度: {} 字节", request_id, text.len());
            text
        }
        Ok(Err(e)) => {
            println!(
                "[{}] 读取响应体失败: {} ({:?})",
                request_id,
                e,
                start_time.elapsed()
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("读取响应体失败: {}", e),
            ));
        }
        Err(_) => {
            println!(
                "[{}] 读取响应体超时 ({:?})",
                request_id,
                start_time.elapsed()
            );
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "读取上游服务器响应超时".to_string(),
            ));
        }
    };

    // 解析JSON
    println!("[{}] 开始解析JSON...", request_id);
    match serde_json::from_str::<ChatResponseJson>(&text) {
        Ok(json) => Ok(json),
        Err(e) => {
            println!(
                "[{}] 解析JSON失败: {} ({:?})",
                request_id,
                e,
                start_time.elapsed()
            );

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
                            format!(
                                "解析响应JSON失败: {}",
                                e
                            ),
                        ));
                    }

                    // 构造一个有效的响应对象
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
                        format!(
                            "解析响应JSON失败: {}",
                            e
                        ),
                    ))
                }
            }
        }
    }
}
