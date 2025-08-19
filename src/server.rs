use crate::handlers::api_handler::{get_embeddings, get_models};
use crate::handlers::chat_completion_handler::{TaskSender, chat_completion};
use crate::models::api_model::AppState;
use axum::Router;
use axum::{
    Json,
    extract::State,
    routing::{get, post},
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

// 创建路由配置
pub fn create_router(app_state: Arc<(Arc<AppState>, TaskSender, TaskSender)>) -> Router {
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

    Router::new()
        .merge(v1_router)
        .merge(no_prefix_router)
        // 并发限制
        .layer(tower::limit::ConcurrencyLimitLayer::new(
            app_state.0.max_concurrent_requests,
        ))
        .with_state(app_state)
}

// 启动服务器函数
pub async fn start_server(app: Router) -> Result<(), Box<dyn std::error::Error>> {
    println!("正在启动服务器...");
    let listener = TcpListener::bind("0.0.0.0:4321").await?;
    println!("服务器正在监听: 4321 端口, 请访问 http://127.0.0.1:4321/v1/chat/completions");

    let server = axum::serve(listener, app.into_make_service());

    println!("服务器已就绪!");

    server.await?;
    Ok(())
}

// 创建任务处理通道和运行时
pub fn create_task_channels(
    cache_hit_pool_size: usize,
    cache_miss_pool_size: usize,
) -> (
    TaskSender,
    TaskSender,
    Arc<tokio::runtime::Runtime>,
    Arc<tokio::runtime::Runtime>,
) {
    // 创建专用线程池
    let hit_runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(cache_hit_pool_size)
            .thread_name("cache-hit-pool")
            .enable_all()
            .build()
            .expect("无法创建缓存命中处理运行时"),
    );

    let miss_runtime = Arc::new(
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

    (tx_hit, tx_miss, hit_runtime, miss_runtime)
}
