use crate::models::api_model::{ChatChoice, ChatMessageJson, ChatResponseJson, Usage};
use axum::http::StatusCode;
use std::time::{Duration, Instant};
use std::sync::OnceLock;

// 全局HTTP客户端
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

fn get_optimized_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(true)
            .pool_max_idle_per_host(20) // 增加连接池大小
            .tcp_keepalive(Some(Duration::from_secs(60)))
            .tcp_nodelay(true) // 启用TCP NoDelay
            .no_proxy()
            .http2_adaptive_window(true) // 使用HTTP/2时自适应窗口大小
            .http2_initial_stream_window_size(1024 * 1024) // 1MB初始窗口大小
            .http2_keep_alive_interval(Some(Duration::from_secs(20)))
            .http2_keep_alive_timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("创建HTTP客户端失败: {}，使用默认配置", e);
                reqwest::Client::new()
            })
    })
}

// 请求超时辅助函数
async fn with_timeout<T, E>(
    duration: Duration,
    future: impl std::future::Future<Output = Result<T, E>>,
    timeout_msg: &'static str,
) -> Result<T, (StatusCode, String)>
where
    E: std::fmt::Display,
{
    match tokio::time::timeout(duration, future).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => {
            // 将错误转换为字符串
            let err_msg = format!("{}", e);
            
            // 根据错误类型返回不同状态码
            if err_msg.contains("connect") || err_msg.contains("connection") {
                Err((
                    StatusCode::BAD_GATEWAY,
                    format!("无法连接到上游服务器: {}", e),
                ))
            } else if err_msg.contains("timeout") {
                Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("上游服务器响应超时: {}", e),
                ))
            } else {
                Err((
                    StatusCode::BAD_GATEWAY,
                    format!("请求上游服务器失败: {}", e),
                ))
            }
        }
        Err(_) => Err((StatusCode::GATEWAY_TIMEOUT, timeout_msg.to_string())),
    }
}

pub async fn send_proxied_request(
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

    // 使用优化的全局客户端
    let optimized_client = get_optimized_client();

    // 创建请求构建器
    let mut request_builder = optimized_client.post(target_url);

    // 设置请求头
    for (key, value) in headers {
        request_builder = request_builder.header(key, value);
    }

    if !headers.contains_key("Content-Type") {
        request_builder = request_builder.header("Content-Type", "application/json");
    }

    println!("[{}] 开始发送请求...", request_id);

    let response = with_timeout(
        Duration::from_secs(30),
        request_builder.body(payload_json.to_owned()).send(),
        "连接上游服务器超时",
    )
    .await?;

    println!(
        "[{}] 成功收到响应: 状态码 = {} ({:?})",
        request_id,
        response.status(),
        start_time.elapsed()
    );

    // 检查响应状态
    if !response.status().is_success() {
        return Err((
            StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            format!("上游服务器返回错误: {:?}", response),
        ));
    }

    // 读取响应体
    println!("[{}] 状态码正常，开始读取响应体...", request_id);
    let text = with_timeout(
        Duration::from_secs(30),
        response.text(),
        "读取上游服务器响应超时",
    )
    .await?;

    println!("[{}] 成功获取响应体，长度: {} 字节", request_id, text.len());

    // 解析JSON
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
                    let choices = extract_choices_from_json(&generic_json);

                    if choices.is_empty() {
                        println!("[{}] 无法从通用JSON中提取有效的消息内容", request_id);
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("解析响应JSON失败: {}", e),
                        ));
                    }

                    let response = construct_response_from_json(generic_json, choices);

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

fn extract_choices_from_json(generic_json: &serde_json::Value) -> Vec<ChatChoice> {
    match generic_json.get("choices") {
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
    }
}

// 从JSON构造响应对象
fn construct_response_from_json(
    generic_json: serde_json::Value,
    choices: Vec<ChatChoice>,
) -> ChatResponseJson {
    ChatResponseJson {
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
    }
}
