use axum::Router;
use axum::{
    Json,
    extract::State,
    routing::{get, post},
};
use llm_api::handlers::api_handler::{get_embeddings, get_models};
use llm_api::handlers::chat_completion_handler::TaskSender;
use llm_api::handlers::chat_completion_handler::chat_completion;
use llm_api::models::api_model::{ApiEndpoint, AppState};
use reqwest;
use serde::{Deserialize, Serialize};
use serde_json;
use sqlx::Executor;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::fs::File;
use std::io::Read;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Semaphore;

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_database_url")]
    database_url: String,
    api_endpoints: Vec<ApiEndpoint>,
    #[serde(default = "default_use_curl")]
    use_curl: bool,
    #[serde(default = "default_use_proxy")]
    use_proxy: bool,
    #[serde(default = "default_cache_hit_pool_size")]
    cache_hit_pool_size: usize,
    #[serde(default = "default_cache_miss_pool_size")]
    cache_miss_pool_size: usize,
    #[serde(default = "default_max_concurrent_requests")]
    max_concurrent_requests: usize,
    #[serde(default = "default_cache_version")]
    cache_version: i32,
    #[serde(default = "default_cache_override_mode")]
    cache_override_mode: bool,
    #[serde(default = "default_api_headers")]
    api_headers: std::collections::HashMap<String, String>,
}

fn default_database_url() -> String {
    "sqlite:cache.db".to_string()
}

fn default_use_curl() -> bool {
    false
}

fn default_use_proxy() -> bool {
    true
}

fn default_cache_hit_pool_size() -> usize {
    8
}

fn default_cache_miss_pool_size() -> usize {
    8
}

fn default_max_concurrent_requests() -> usize {
    100
}

fn default_cache_version() -> i32 {
    0
}

fn default_cache_override_mode() -> bool {
    false
}

// 默认头信息函数
fn default_api_headers() -> std::collections::HashMap<String, String> {
    let mut headers = std::collections::HashMap::new();
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers
}

// 初始化数据库
async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cache (
            key TEXT PRIMARY KEY,
            response BLOB NOT NULL,
            size INTEGER NOT NULL,
            hit_count INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::main]
async fn main() {
    let mut file = File::open("config.yaml").expect("无法打开配置文件");
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .expect("无法读取配置文件");
    let config: Config = serde_yaml::from_str(&contents).expect("解析配置文件失败");

    println!(
        "服务配置: 数据库={}, 使用curl={}, 最大并发请求={}",
        config.database_url, config.use_curl, config.max_concurrent_requests
    );

    let pool = SqlitePoolOptions::new()
        .max_connections(100)
        .min_connections(5)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&config.database_url)
                .create_if_missing(true),
        )
        .await
        .expect("无法打开数据库");

    init_db(&pool).await.expect("数据库初始化失败");

    // 数据库优化参数
    let pragmas = [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA wal_autocheckpoint=4;",
        "PRAGMA wal_checkpoint(FULL);",
        "PRAGMA read_uncommitted=true;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA cache_size=10000;",
        "PRAGMA temp_store=MEMORY;",
        "PRAGMA mmap_size=30000000000;",
    ];

    for pragma in pragmas.iter() {
        if let Err(e) = pool.execute(*pragma).await {
            eprintln!("设置SQLite参数失败 ({}): {}", pragma, e);
        }
    }

    let hit_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.cache_hit_pool_size)
            .thread_name("cache-hit-pool")
            .enable_all()
            .build()
            .expect("无法创建缓存命中处理运行时"),
    );

    let miss_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.cache_miss_pool_size)
            .thread_name("cache-miss-pool")
            .enable_all()
            .build()
            .expect("无法创建缓存未命中处理运行时"),
    );

    println!(
        "已创建专门处理缓存命中的线程池，线程数: {}",
        config.cache_hit_pool_size
    );
    println!(
        "已创建专门处理缓存未命中的线程池，线程数: {}",
        config.cache_miss_pool_size
    );

    println!("创建HTTP客户端...");
    // 明确创建不使用任何代理的HTTP客户端
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_nodelay(true)
        .pool_idle_timeout(std::time::Duration::from_secs(120))
        .pool_max_idle_per_host(30)
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .http1_title_case_headers()
        .no_proxy() // 禁用代理
        .build()
        .expect("无法创建HTTP客户端");

    let shared_state = Arc::new(AppState {
        db: Arc::new(pool),
        client: http_client,
        api_endpoints: config.api_endpoints.clone(),
        max_concurrent_requests: config.max_concurrent_requests,
        semaphore: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        cache_version: config.cache_version,
        cache_override_mode: config.cache_override_mode,
        use_curl: config.use_curl,
        use_proxy: config.use_proxy,
        api_headers: config.api_headers,
    });

    // 处理命中缓存请求的通道
    let (tx_hit, mut rx_hit) = mpsc::channel(2048);

    // 处理未命中缓存请求的通道
    let (tx_miss, mut rx_miss) = mpsc::channel(2048);

    // 处理缓存命中的后台任务
    let hit_runtime_clone = hit_runtime.clone();
    tokio::spawn(async move {
        while let Some(task) = rx_hit.recv().await {
            hit_runtime_clone.spawn(task);
        }
    });

    // 处理缓存未命中的后台任务
    let miss_runtime_clone = miss_runtime.clone();
    tokio::spawn(async move {
        while let Some(task) = rx_miss.recv().await {
            miss_runtime_clone.spawn(task);
        }
    });

    let app_state = Arc::new((shared_state.clone(), tx_hit, tx_miss));

    let v1_router = Router::new()
        .route("/v1/chat/completions", post(chat_completion))
        .route(
            "/v1/models",
            get(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 headers: axum::http::HeaderMap| async move {
                    get_models(State(state.0.0.clone()), headers).await
                },
            ),
        )
        .route(
            "/v1/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 headers: axum::http::HeaderMap,
                 payload: Json<serde_json::Value>| async move {
                    get_embeddings(State(state.0.0.clone()), headers, payload).await
                },
            ),
        );

    let no_prefix_router = Router::new()
        .route("/chat/completions", post(chat_completion))
        .route(
            "/models",
            get(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 headers: axum::http::HeaderMap| async move {
                    get_models(State(state.0.0.clone()), headers).await
                },
            ),
        )
        .route(
            "/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 headers: axum::http::HeaderMap,
                 payload: Json<serde_json::Value>| async move {
                    get_embeddings(State(state.0.0.clone()), headers, payload).await
                },
            ),
        );

    let app = Router::new()
        .merge(v1_router)
        .merge(no_prefix_router)
        // 并发限制
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            config.max_concurrent_requests,
        ))
        .with_state(app_state);

    println!("正在启动服务器...");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4321").await.unwrap();
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");

    let server = axum::serve(listener, app.into_make_service());

    println!("服务器已就绪!");

    server.await.unwrap();
}
