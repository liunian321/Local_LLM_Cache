// 载入依赖与模块
use axum::Router;
use dotenv::dotenv;
use llm_api::handlers::api_handler::{chat_completion, get_embeddings, get_models};
use llm_api::models::api_model::AppState;
use reqwest;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::Executor;
use std::{env, sync::Arc};
use tokio::runtime::Runtime;

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
    let database_url: String = env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:cache.db".to_string());
    let api_url: String =
        env::var("API_URL").unwrap_or_else(|_| "http://127.0.0.1:1234".to_string());
    let use_curl: bool = env::var("USE_CURL")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);
    // 读取缓存未命中线程池大小，默认为 4
    let cache_miss_pool_size: usize = env::var("CACHE_MISS_POOL_SIZE")
        .unwrap_or_else(|_| "4".to_string())
        .parse()
        .expect("无法解析 CACHE_MISS_POOL_SIZE");
    let cache_hit_pool_size: usize = env::var("CACHE_HIT_POOL_SIZE")
        .unwrap_or_else(|_| "64".to_string())
        .parse()
        .expect("无法解析 CACHE_HIT_POOL_SIZE");

    println!(
        "服务配置: 数据库={}, API地址={}, 使用curl={}, 未命中池大小={}",
        database_url, api_url, use_curl, cache_miss_pool_size
    );

    // 打开 SQLite 连接池
    let pool = SqlitePoolOptions::new()
        .max_connections(10) // 设置最大连接数
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database_url)
                .create_if_missing(true),
        )
        .await
        .expect("无法打开数据库");

    // 初始化数据库
    init_db(&pool).await.expect("数据库初始化失败");

    pool.execute("PRAGMA journal_mode=WAL;").await.expect("无法启用 WAL 模式");

    pool.execute("PRAGMA wal_autocheckpoint=4;").await.expect("无法设置自动检查点");

    pool.execute("PRAGMA wal_checkpoint(FULL);").await.expect("无法执行检查点");

    // 专门用于处理缓存未命中的运行时
    let miss_runtime: Arc<Runtime> = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cache_miss_pool_size) // 工作线程数上限
            .thread_name("cache-miss-pool")
            .enable_all() // 启用所有 Tokio 功能
            .build()
            .expect("无法创建缓存未命中处理运行时"),
    );

    // 专门用于处理命中缓存的运行时
    let hit_runtime: Arc<Runtime> = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cache_hit_pool_size) // 工作线程数上限
            .thread_name("cache-hit-pool")
            .enable_all() // 启用所有 Tokio 功能
            .build()
            .expect("无法创建缓存命中处理运行时"),
    );

    println!(
        "已创建专门处理缓存未命中的线程池，线程数: {}, 专门处理缓存命中的线程池，线程数: {}",
        cache_miss_pool_size, cache_hit_pool_size
    );

    // 构造共享状态, 加入 miss_runtime 和 hit_runtime
    let shared_state: Arc<AppState> = Arc::new(AppState {
        db: Arc::new(pool), // 使用 Arc 包装 SqlitePool
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
        miss_runtime,
        hit_runtime,
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

    // 监听本地 4321 端口
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4321").await.unwrap();
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
