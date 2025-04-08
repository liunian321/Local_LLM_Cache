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

    println!(
        "服务配置: 数据库={}, API地址={}, 使用curl={}",
        database_url, api_url, use_curl
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

    pool.execute("PRAGMA journal_mode=WAL;")
        .await
        .expect("无法启用 WAL 模式");

    pool.execute("PRAGMA wal_autocheckpoint=4;")
        .await
        .expect("无法设置自动检查点");

    pool.execute("PRAGMA wal_checkpoint(FULL);")
        .await
        .expect("无法执行检查点");
    pool.execute("PRAGMA read_uncommitted=true;")
        .await
        .expect("无法设置读未提交");

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
        "已创建专门处理缓存未命中的线程池，线程数: {}",
        cache_hit_pool_size
    );

    let shared_state: Arc<AppState> = Arc::new(AppState {
        db: Arc::new(pool),
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1800))
            .pool_idle_timeout(std::time::Duration::from_secs(600))
            .http2_prior_knowledge() // 启用 HTTP/2
            .danger_accept_invalid_certs(true) // 接受自签名证书
            .http1_title_case_headers() // 使用标题大小写形式的头
            .no_proxy() // 禁用代理
            .build()
            .expect("无法创建HTTP客户端"),
        api_url,
    });

    // 创建专用于处理请求的通道
    let (tx_hit, mut rx_hit) = mpsc::channel(2048);

    // 启动处理缓存命中的后台任务
    let hit_runtime_clone = hit_runtime.clone();
    tokio::spawn(async move {
        while let Some(task) = rx_hit.recv().await {
            hit_runtime_clone.spawn(task);
        }
    });

    // 将通道发送端添加到应用状态
    let app_state = Arc::new((shared_state, tx_hit));

    // 构建 Axum 路由
    let v1_router = Router::new()
        .route("/v1/chat/completions", post(chat_completion))
        .route(
            "/v1/models",
            get(
                |state: State<Arc<(Arc<AppState>, TaskSender)>>| async move {
                    get_models(State(state.0.0.clone())).await
                },
            ),
        )
        .route(
            "/v1/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender)>>,
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
                |state: State<Arc<(Arc<AppState>, TaskSender)>>| async move {
                    get_models(State(state.0.0.clone())).await
                },
            ),
        )
        .route(
            "/embeddings",
            post(
                |state: State<Arc<(Arc<AppState>, TaskSender)>>,
                 payload: Json<serde_json::Value>| async move {
                    get_embeddings(State(state.0.0.clone()), payload).await
                },
            ),
        );

    let app = Router::new()
        .merge(v1_router)
        .merge(no_prefix_router)
        .layer(tower::limit::ConcurrencyLimitLayer::new(128)) // 允许128个并发请求
        .with_state(app_state);

    // 监听本地 4321 端口
    let listener = tokio::net::TcpListener::bind("0.0.0.0:4321").await.unwrap();
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
