// 载入依赖与模块
use axum::Router;
use axum::{
    Json,
    extract::State,
    routing::{get, post},
};
use dotenv::dotenv;
use llm_api::handlers::api_handler::{get_embeddings, get_models};
use llm_api::handlers::chat_completion_handler::TaskSender;
use llm_api::handlers::chat_completion_handler::chat_completion;
use llm_api::models::api_model::AppState;
use num_cpus;
use reqwest;
use serde_json;
use sqlx::Executor;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{env, sync::Arc};
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

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
    dotenv().ok();
    // 从环境变量加载数据库与 API 地址
    let database_url: String =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:cache.db".to_string());
    let api_url: String =
        env::var("API_URL").unwrap_or_else(|_| "http://127.0.0.1:1234".to_string());
    let use_curl: bool = env::var("USE_CURL")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);
    let cache_hit_pool_size: usize = env::var("CACHE_HIT_POOL_SIZE")
        .unwrap_or_else(|_| (num_cpus::get() * 2).to_string())
        .parse()
        .expect("无法解析 CACHE_HIT_POOL_SIZE");
    let cache_miss_pool_size: usize = env::var("CACHE_MISS_POOL_SIZE")
        .unwrap_or_else(|_| (num_cpus::get() * 4).to_string())
        .parse()
        .expect("无法解析 CACHE_MISS_POOL_SIZE");
    let max_concurrent_requests: usize = env::var("MAX_CONCURRENT_REQUESTS")
        .unwrap_or_else(|_| "100".to_string())
        .parse()
        .expect("无法解析 MAX_CONCURRENT_REQUESTS");

    println!(
        "服务配置: 数据库={}, API地址={}, 使用curl={}, 最大并发请求={}",
        database_url, api_url, use_curl, max_concurrent_requests
    );

    // 打开 SQLite 连接池
    let pool = SqlitePoolOptions::new()
        .max_connections(100)
        .min_connections(5) // 最小连接数
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database_url)
                .create_if_missing(true),
        )
        .await
        .expect("无法打开数据库");

    // 初始化数据库
    init_db(&pool).await.expect("数据库初始化失败");

    // 配置数据库优化参数
    let pragmas = [
        "PRAGMA journal_mode=WAL;",
        "PRAGMA wal_autocheckpoint=4;",
        "PRAGMA wal_checkpoint(FULL);",
        "PRAGMA read_uncommitted=true;",
        "PRAGMA synchronous=NORMAL;",
        "PRAGMA cache_size=10000;",
        "PRAGMA temp_store=MEMORY;",
        "PRAGMA mmap_size=30000000000;"
    ];

    for pragma in pragmas.iter() {
        if let Err(e) = pool.execute(*pragma).await {
            eprintln!("设置SQLite参数失败 ({}): {}", pragma, e);
        }
    }

    // 专门用于处理命中缓存的运行时
    let hit_runtime: Arc<Runtime> = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cache_hit_pool_size) 
            .thread_name("cache-hit-pool")
            .enable_all() 
            .build()
            .expect("无法创建缓存命中处理运行时"),
    );

    // 专门用于处理未命中缓存的运行时
    let miss_runtime: Arc<Runtime> = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cache_miss_pool_size) 
            .thread_name("cache-miss-pool")
            .enable_all() 
            .build()
            .expect("无法创建缓存未命中处理运行时"),
    );

    println!(
        "已创建专门处理缓存命中的线程池，线程数: {}",
        cache_hit_pool_size
    );
    println!(
        "已创建专门处理缓存未命中的线程池，线程数: {}",
        cache_miss_pool_size
    );

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(1800))
        // .http2_prior_knowledge() // 启用 HTTP/2
        .pool_max_idle_per_host(max_concurrent_requests) // 最大保持连接数
        .tcp_keepalive(Some(std::time::Duration::from_secs(60))) // TCP连接保活
        .pool_idle_timeout(std::time::Duration::from_secs(600)) // 闲置连接超时时间
        .tcp_nodelay(true) // 设置TCP_NODELAY
        .danger_accept_invalid_certs(true) // 接受自签名证书
        .build()
        .expect("无法创建HTTP客户端");

    let shared_state: Arc<AppState> = Arc::new(AppState {
        db: Arc::new(pool),
        client: http_client,
        api_url,
        max_concurrent_requests,
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

    // 将通道发送端添加到应用状态
    let app_state = Arc::new((shared_state, tx_hit, tx_miss));

    // 构建 Axum 路由
    let v1_router = Router::new()
        .route("/v1/chat/completions", post(chat_completion))
        .route(
            "/v1/models",
            get(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>| async move {
                    get_models(State(state.0.0.clone())).await
                },
            ),
        )
        .route(
            "/v1/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 payload: Json<serde_json::Value>| async move {
                    get_embeddings(State(state.0.0.clone()), payload).await
                },
            ),
        );

    let no_prefix_router = Router::new()
        .route("/chat/completions", post(chat_completion))
        .route(
            "/models",
            get(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>| async move {
                    get_models(State(state.0.0.clone())).await
                },
            ),
        )
        .route(
            "/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender, TaskSender)>>,
                 payload: Json<serde_json::Value>| async move {
                    get_embeddings(State(state.0.0.clone()), payload).await
                },
            ),
        );

    let app = Router::new()
        .merge(v1_router)
        .merge(no_prefix_router)
        // 并发限制
        .layer(tower::limit::ConcurrencyLimitLayer::new(max_concurrent_requests)) 
        .with_state(app_state);

    println!("正在启动服务器...");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4321").await.unwrap();
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");
    
    let server = axum::serve(listener, app.into_make_service());
    
    println!("服务器已就绪，可以接收并发请求!");
    
    server.await.unwrap();
}
