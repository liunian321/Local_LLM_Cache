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
use tokio::sync::Semaphore;
use tokio::sync::mpsc;

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

// 优化数据库初始化和配置
async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    // 创建缓存表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS cache (
            key TEXT PRIMARY KEY,
            response BLOB NOT NULL,
            size INTEGER NOT NULL,
            hit_count INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
        )",
    )
    .execute(pool)
    .await?;

    // 创建索引以提高查询速度
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_cache_key ON cache(key)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_cache_version ON cache(version)")
        .execute(pool)
        .await?;

    Ok(())
}

// 优化数据库参数
async fn optimize_db(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    // 数据库优化参数
    let pragmas = [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA wal_autocheckpoint=1000;", // 增加检查点间隔以提高写入性能
        "PRAGMA wal_checkpoint(PASSIVE);", // 使用被动检查点避免阻塞
        "PRAGMA read_uncommitted=true;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA cache_size=20000;", // 增加缓存大小
        "PRAGMA temp_store=MEMORY;",
        "PRAGMA mmap_size=30000000000;",
        "PRAGMA page_size=4096;",    // 使用更高效的页大小
        "PRAGMA busy_timeout=5000;", // 设置忙等待超时
        "PRAGMA foreign_keys=OFF;",  // 关闭外键约束检查以提高性能
    ];

    for pragma in pragmas.iter() {
        match pool.execute(*pragma).await {
            Ok(_) => {},
            Err(e) => {
                eprintln!("设置SQLite参数失败 ({}): {}", pragma, e);
            }
        }
    }

    // 运行一次VACUUM来整理数据库
    match pool.execute("VACUUM;").await {
        Ok(_) => println!("数据库VACUUM成功"),
        Err(e) => eprintln!("数据库VACUUM失败: {}", e),
    }

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

    // 使用优化的连接选项
    let pool = SqlitePoolOptions::new()
        .max_connections(100)
        .min_connections(10) // 增加最小连接数，降低连接启动开销
        .max_lifetime(std::time::Duration::from_secs(1800)) // 连接最长生命周期30分钟
        .idle_timeout(std::time::Duration::from_secs(600)) // 闲置超时10分钟
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&config.database_url)
                .create_if_missing(true)
                .foreign_keys(false) // 禁用外键约束检查以提高性能
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal) // 使用WAL模式
                .synchronous(sqlx::sqlite::SqliteSynchronous::Normal), // 降低同步级别
        )
        .await
        .expect("无法打开数据库");

    init_db(&pool).await.expect("数据库初始化失败");
    optimize_db(&pool).await.expect("数据库优化失败");

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
    // 优化HTTP客户端配置
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .tcp_nodelay(true)
        .tcp_keepalive(Some(std::time::Duration::from_secs(60)))
        .pool_idle_timeout(std::time::Duration::from_secs(180)) // 增加空闲连接超时
        .pool_max_idle_per_host(50) // 增加每个主机最大空闲连接数
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .http1_title_case_headers()
        .http2_adaptive_window(true) // 启用HTTP/2自适应窗口大小
        .http2_keep_alive_interval(Some(std::time::Duration::from_secs(30)))
        .http2_keep_alive_timeout(std::time::Duration::from_secs(30))
        .http2_initial_stream_window_size(1024 * 1024) // 1MB窗口大小
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
