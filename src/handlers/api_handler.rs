use crate::models::api_model::{
    AppState, ChatChoice, ChatMessageJson, ChatResponseJson, Usage, select_api_endpoint,
};
use axum::{
    extract::{Json, State},
    http::StatusCode,
};
use std::sync::Arc;

// 使用 curl 发送请求函数
pub async fn send_request_with_curl(
    url: &str,
    payload: &str,
) -> Result<ChatResponseJson, (StatusCode, String)> {
    // 使用较短的超时设置，避免长时间阻塞
    let curl_command = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::process::Command::new("curl")
            .arg("-sS") // 静默模式，但显示错误
            .arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("-H")
            .arg("Accept: application/json")
            .arg("-H")
            .arg("User-Agent: llm_api_rust_client/1.0")
            .arg("--connect-timeout")
            .arg("5") // 连接超时5秒
            .arg("--max-time")
            .arg("10") // 总超时10秒
            .arg("-d")
            .arg(payload)
            .arg(url)
            .output(),
    )
    .await;

    // 处理 tokio 超时
    let curl_output = match curl_command {
        Ok(output_result) => match output_result {
            Ok(output) => output,
            Err(e) => {
                println!("curl命令执行失败: {}", e);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("curl命令执行失败: {}", e),
                ));
            }
        },
        Err(_) => {
            println!("curl命令执行超时");
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "curl命令执行超时，请检查 API URL 是否正确".to_string(),
            ));
        }
    };

    // 处理 curl 执行结果
    if !curl_output.status.success() {
        let stderr = String::from_utf8_lossy(&curl_output.stderr);
        let stdout = String::from_utf8_lossy(&curl_output.stdout);

        // 检查是否包含常见错误
        if stderr.contains("timed out") || stderr.contains("Connection refused") {
            println!("curl连接失败: {}", stderr);
            return Err((
                StatusCode::BAD_GATEWAY,
                format!("无法连接到上游服务器: {}", stderr),
            ));
        }

        eprintln!("curl命令失败: stderr={}, stdout={}", stderr, stdout);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("curl命令失败 (状态码={})", curl_output.status),
        ));
    }

    // 解析响应
    let response_text = String::from_utf8_lossy(&curl_output.stdout).to_string();

    match serde_json::from_str::<ChatResponseJson>(&response_text) {
        Ok(response) => Ok(response),
        Err(e) => {
            match serde_json::from_str::<serde_json::Value>(&response_text) {
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
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("解析curl响应失败: {}", e),
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

                    Ok(response)
                }
                Err(parse_err) => {
                    println!("解析为通用JSON也失败: {}", parse_err);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("解析curl响应失败: {}", e),
                    ))
                }
            }
        }
    }
}

// 处理 /v1/models 路由的请求
pub async fn get_models(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<String, (StatusCode, String)> {
    // 选择 API 端点
    let endpoint = match select_api_endpoint(&state.api_endpoints) {
        Some(ep) => ep,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "没有可用的 API 端点".to_string(),
            ));
        }
    };

    let target_url = if endpoint.url.ends_with('/') {
        format!("{}v1/models", endpoint.url)
    } else {
        format!("{}/v1/models", endpoint.url)
    };

    // 创建新的客户端，设置短超时
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut req_builder = client.get(&target_url);

    // 添加所有请求头
    for (key, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            req_builder = req_builder.header(key.as_str(), v);
        }
    }

    // 使用 tokio timeout 包装请求
    let response =
        match tokio::time::timeout(std::time::Duration::from_secs(10), req_builder.send()).await {
            Ok(result) => match result {
                Ok(res) => res,
                Err(e) => {
                    println!("模型列表请求失败: {}", e);
                    // 更详细的错误类型判断
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
            },
            Err(_) => {
                println!("模型列表请求超时");
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    "请求上游服务器超时，请检查 API URL 是否正确".to_string(),
                ));
            }
        };

    if !response.status().is_success() {
        return Err((
            response.status(),
            format!("上游服务器返回错误: {:?}", response),
        ));
    }

    // 添加响应读取超时
    let response_text =
        match tokio::time::timeout(std::time::Duration::from_secs(5), response.text()).await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("读取响应失败: {}", e),
                ));
            }
            Err(_) => {
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    "读取上游服务器响应超时".to_string(),
                ));
            }
        };

    Ok(response_text)
}

// 处理 /v1/embeddings 路由的请求
pub async fn get_embeddings(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<String, (StatusCode, String)> {
    // 选择 API 端点
    let endpoint = match select_api_endpoint(&state.api_endpoints) {
        Some(ep) => ep,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "没有可用的 API 端点".to_string(),
            ));
        }
    };

    let target_url = if endpoint.url.ends_with('/') {
        format!("{}v1/embeddings", endpoint.url)
    } else {
        format!("{}/v1/embeddings", endpoint.url)
    };

    // 创建新的客户端，设置短超时
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut req_builder = client.post(&target_url);

    // 添加所有请求头
    for (key, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            req_builder = req_builder.header(key.as_str(), v);
        }
    }

    // 使用 tokio timeout 包装请求
    let response = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        req_builder.json(&payload).send(),
    )
    .await
    {
        Ok(result) => match result {
            Ok(res) => res,
            Err(e) => {
                println!("嵌入请求失败: {}", e);
                // 更详细的错误类型判断
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
        },
        Err(_) => {
            println!("嵌入请求超时");
            return Err((
                StatusCode::GATEWAY_TIMEOUT,
                "请求上游服务器超时，请检查 API URL 是否正确".to_string(),
            ));
        }
    };

    if !response.status().is_success() {
        return Err((
            response.status(),
            format!("上游服务器返回错误: {:?}", response),
        ));
    }

    // 添加响应读取超时
    let response_text =
        match tokio::time::timeout(std::time::Duration::from_secs(5), response.text()).await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("读取响应失败: {}", e),
                ));
            }
            Err(_) => {
                return Err((
                    StatusCode::GATEWAY_TIMEOUT,
                    "读取上游服务器响应超时".to_string(),
                ));
            }
        };

    Ok(response_text)
}
