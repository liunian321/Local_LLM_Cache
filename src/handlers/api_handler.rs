use crate::models::api_model::{AppState, ChatResponseJson};
use axum::{
    extract::{Json, State},
    http::StatusCode,
};
use std::sync::Arc;

// 使用 curl 发送请求的函数
pub async fn send_request_with_curl(
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
