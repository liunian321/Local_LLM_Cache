// 载入依赖与模块
use axum::{
    Router,
    extract::{Json, State},
    http::StatusCode,
};
use brotli::CompressorWriter;
use dotenv::dotenv;
use reqwest;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::{env, sync::Arc};
use tokio::sync::Mutex; // 引入 Write trait

// 定义对外暴露的请求 JSON 数据结构
#[derive(Debug, Deserialize, Serialize)]
struct ChatRequestJson {
    model: String,
    messages: Vec<ChatMessageJson>,
    #[serde(default = "default_temperature")]
    temperature: f32,
    #[serde(default = "default_max_tokens")]
    max_tokens: i32,
    #[serde(default = "default_stream")]
    stream: bool,
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

// 定义返回给客户端的响应 JSON 结构
#[derive(Debug, Serialize, Deserialize)]
struct ChatResponseJson {
    id: String,
    object: String,
    created: i64,
    model: String,
    choices: Vec<ChatChoice>,
    usage: Usage,
    stats: serde_json::Value,
    system_fingerprint: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatChoice {
    index: i32,
    logprobs: Option<serde_json::Value>,
    finish_reason: String,
    message: ChatMessageJson,
}

#[derive(Debug, Serialize, Deserialize)]
struct Usage {
    prompt_tokens: i32,
    completion_tokens: i32,
    total_tokens: i32,
}

// 为 ChatMessageJson 结构体添加 Serialize 派生宏
#[derive(Debug, Serialize, Deserialize)]
struct ChatMessageJson {
    role: String,
    content: String,
}

// 修改 AppState 结构体，将 db 包装在 Arc 中
struct AppState {
    db: Arc<Mutex<Connection>>, // 使用 Arc 包装 Mutex
    client: reqwest::Client,
    api_url: String,
}

// 修改 init_db 函数，新增 size 和 hit_count 字段
fn init_db(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS cache (
            key TEXT PRIMARY KEY,
            response BLOB NOT NULL,
            size INTEGER NOT NULL,
            hit_count INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;
    Ok(())
}

#[tokio::main]
async fn main() {
    dotenv().ok();
    // 从环境变量加载数据库与 API 地址
    let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "cache.db".to_string());
    let api_url = env::var("API_URL").unwrap_or_else(|_| "http://127.0.0.1:1234".to_string());
    let use_curl = env::var("USE_CURL").unwrap_or_else(|_| "false".to_string()).parse::<bool>().unwrap_or(false);

    println!("服务配置: 数据库={}, API地址={}, 使用curl={}", database_url, api_url, use_curl);

    // 打开 SQLite 连接
    let conn = Connection::open(database_url).expect("无法打开数据库");

    // 启用 WAL 模式
    let wal_mode: String = conn
        .query_row("PRAGMA journal_mode=WAL;", [], |row| row.get(0))
        .expect("无法启用 WAL 模式");

    // 检查 WAL 模式是否成功启用
    if wal_mode.to_lowercase() != "wal" {
        panic!("WAL 模式启用失败，当前模式为: {}", wal_mode);
    }

    // 设置自动检查点，每 1000 页执行一次检查点
    conn.query_row("PRAGMA wal_autocheckpoint=4;", [], |_| Ok(()))
        .expect("无法设置自动检查点");

    // 定期执行检查点操作
    let checkpoint_result: (i32, i32, i32) = conn
        .query_row("PRAGMA wal_checkpoint(FULL);", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("无法执行检查点");

    println!("检查点执行结果: {:?}", checkpoint_result);

    init_db(&conn).expect("数据库初始化失败");

    // 构造共享状态
    let shared_state = Arc::new(AppState {
        db: Arc::new(Mutex::new(conn)), // 使用 Arc 包装 Mutex
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(600))
            .connect_timeout(std::time::Duration::from_secs(30))
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .danger_accept_invalid_certs(true) // 接受自签名证书
            .http1_title_case_headers() // 使用标题大小写形式的头
            .no_proxy() // 禁用代理
            .build()
            .expect("无法创建HTTP客户端"),
        api_url,
    });

    // 构建 Axum 路由
    let v1_router = Router::new()
        .route("/v1/chat/completions", axum::routing::post(chat_completion))
        .route("/v1/models", axum::routing::get(get_models))
        .route("/v1/embeddings", axum::routing::post(get_embeddings));

    let no_prefix_router = Router::new()
        .route("/chat/completions", axum::routing::post(chat_completion))
        .route("/models", axum::routing::get(get_models))
        .route("/embeddings", axum::routing::post(get_embeddings));

    let app = Router::new()
        .merge(v1_router)
        .merge(no_prefix_router)
        .with_state(shared_state);

    // 启动 HTTP 服务监听本地 4321 端口
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4321").await.unwrap();
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[axum::debug_handler]
async fn chat_completion(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ChatRequestJson>,
) -> Result<Json<ChatResponseJson>, (StatusCode, String)> {
    // 记录请求开始时间
    let start_time = std::time::Instant::now();

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
    let cached_result = tokio::task::spawn_blocking(move || {
        let conn = db_arc.blocking_lock();
        if cache_override_mode {
            conn.prepare("SELECT response FROM cache WHERE key = ?1 AND version = ?2")
                .and_then(|mut stmt| {
                    stmt.query_row(params![key, cache_version], |row: &rusqlite::Row<'_>| {
                        row.get::<_, Vec<u8>>(0)
                    })
                })
                .optional()
        } else {
            conn.prepare("SELECT response FROM cache WHERE key = ?1")
                .and_then(|mut stmt| {
                    stmt.query_row(params![key], |row: &rusqlite::Row<'_>| {
                        row.get::<_, Vec<u8>>(0)
                    })
                })
                .optional()
        }
    })
    .await;

    // 处理 cached_result (它会是 Result<Result<Option<Vec<u8>>, rusqlite::Error>, tokio::task::JoinError>)
    let cached = match cached_result {
        Ok(Ok(data)) => data, // 成功获取到数据库结果
        Ok(Err(e)) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("数据库查询错误: {}", e),
            ));
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("执行阻塞任务失败: {}", e),
            ));
        } // spawn_blocking 本身失败
    };

    if let Some(compressed_response) = cached {
        // 缓存命中，使用 brotli 解压缩数据
        let mut decompressed = Vec::new();
        let mut decompressor =
            brotli::Decompressor::new(&compressed_response[..], compressed_response.len());
        std::io::copy(&mut decompressor, &mut decompressed)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // 将解压缩后的数据反序列化为字符串
        let message_content = String::from_utf8(decompressed)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // 异步更新缓存命中次数
        let db_arc = state.db.clone();
        let key = cache_key.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db_arc.blocking_lock();
            if let Err(e) = conn.execute(
                "UPDATE cache SET hit_count = hit_count + 1 WHERE key = ?1",
                params![key],
            ) {
                eprintln!("更新缓存命中次数失败: {}", e);
            }
        });

        // 组装响应结果
        let response = ChatResponseJson {
            id: "chatcmpl-1234567890".to_string(),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp(),
            model: payload.model.clone(),
            choices: vec![ChatChoice {
                index: 0,
                logprobs: None,
                finish_reason: "stop".to_string(),
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
            system_fingerprint: "".to_string(),
        };

        // 打印缓存命中耗时
        let duration = start_time.elapsed();
        println!("缓存命中耗时: {:?}", duration);

        return Ok(Json(response));
    } else {
        // 缓存未命中，调用上游 API
        let target_url = format!("{}/v1/chat/completions", state.api_url);
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("序列化请求负载失败: {}", e)))?;

        // 手动构建请求，避免自动设置头
        let mut request = reqwest::Request::new(
            reqwest::Method::POST,
            reqwest::Url::parse(&target_url)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("解析URL失败: {}", e)))?,
        );

        // 设置请求体
        *request.body_mut() = Some(reqwest::Body::from(payload_json.clone()));

        // 手动设置必要的头信息
        let headers = request.headers_mut();
        headers.insert("Content-Type", "application/json".parse().unwrap());
        headers.insert("Accept", "application/json".parse().unwrap());

        // 从URL提取主机名并设置Host头
        if let Some(host) = reqwest::Url::parse(&target_url).ok().and_then(|u| u.host_str().map(String::from)) {
            headers.insert("Host", host.parse().unwrap());
        }

        // 设置自定义User-Agent
        headers.insert("User-Agent", "llm_api_rust_client/1.0".parse().unwrap());

        // 发送请求
        let res = match state.client.execute(request).await {
            Ok(response) => response,
            Err(e) => {
                println!("使用reqwest客户端请求失败: {}, 尝试使用curl作为备选", e);

                // 检查是否启用了curl选项
                let use_curl = env::var("USE_CURL").unwrap_or_else(|_| "false".to_string()).parse::<bool>().unwrap_or(false);
                if use_curl {
                    // 使用curl发送请求
                    return send_request_with_curl(&target_url, &payload_json).await;
                } else {
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("请求上游API失败: {}", e)));
                }
            }
        };

        // 检查上游 API 的响应状态码
        let status = res.status();
        if !status.is_success() {
            let error_body = res.text().await.unwrap_or_else(|_| "无法读取错误响应体".to_string());
            println!("上游API错误响应体: {}", error_body);
            return Err((
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                format!("上游API返回错误: 状态码 = {}, 错误信息 = {}", status, error_body),
            ));
        }

        // 打印上游 API 的响应体
        let response_text = res
            .text()
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // 尝试反序列化响应体
        let response_json: ChatResponseJson = serde_json::from_str(&response_text)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        // 检查 choices 数组是否为空
        if response_json.choices.is_empty() {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "上游 API 返回的 choices 数组为空".to_string(),
            ));
        }

        // 检查 message 是否存在
        let message = &response_json.choices[0].message;
        if message.content.is_empty() {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "上游 API 返回的 message 内容为空".to_string(),
            ));
        }

        let message_content = message.content.clone();

        // 提取 AI 消息内容并压缩
        let message_bytes = message_content.as_bytes();
        let mut compressed = Vec::new();
        let mut compressor = CompressorWriter::new(&mut compressed, 4096, 11, 22);
        compressor
            .write_all(message_bytes)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        compressor
            .flush()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        drop(compressor);

        // 将压缩后的响应存入 SQLite 缓存表，同时记录大小和命中次数
        let db_arc = state.db.clone();
        let key = cache_key.clone();
        let data_to_insert = compressed.clone();
        let data_size = compressed.len();
        let insert_result = tokio::task::spawn_blocking(move || {
            let conn = db_arc.blocking_lock();
            conn.execute(
                "INSERT OR REPLACE INTO cache (key, response, size, hit_count, version) VALUES (?1, ?2, ?3, 0, ?4)",
                params![key, data_to_insert, data_size, cache_version],
            ).map_err(|e| e.to_string())
        }).await;

        match insert_result {
            Ok(Ok(_)) => (),
            Ok(Err(e)) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("数据库操作错误: {}", e),
                ));
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("执行阻塞任务失败: {}", e),
                ));
            }
        }

        // 打印正常请求第三方接口耗时
        let duration = start_time.elapsed();
        println!("正常请求第三方接口耗时: {:?}", duration);

        return Ok(Json(response_json));
    }
}

// 使用curl作为备选发送请求的函数
async fn send_request_with_curl(url: &str, payload: &str) -> Result<Json<ChatResponseJson>, (StatusCode, String)> {
    // println!("使用curl发送请求到: {}", url);
    // println!("请求体: {}", payload);
    
    // 使用tokio::process运行curl命令
    let curl_output = tokio::process::Command::new("curl")
        .arg("-v") // 详细输出
        .arg("-X").arg("POST")
        .arg("-H").arg("Content-Type: application/json")
        .arg("-H").arg("Accept: application/json")
        .arg("-d").arg(payload)
        .arg(url)
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("执行curl命令失败: {}", e)))?;
    
    // 检查命令是否成功执行
    if !curl_output.status.success() {
        let stderr = String::from_utf8_lossy(&curl_output.stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("curl命令执行失败: {}", stderr),
        ));
    }
    
    // 解析响应
    let response_text = String::from_utf8_lossy(&curl_output.stdout);
    // println!("curl响应: {}", response_text);
    
    // 解析为ChatResponseJson
    let response_json: ChatResponseJson = serde_json::from_str(&response_text)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("解析curl响应失败: {}", e)))?;
    
    Ok(Json(response_json))
}

// 处理 /v1/models 路由的请求
async fn get_models(State(state): State<Arc<AppState>>) -> Result<String, (StatusCode, String)> {
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
async fn get_embeddings(
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
