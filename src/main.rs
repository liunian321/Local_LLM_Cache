use llm_api::models::api_model::AppState;
use llm_api::server::{create_router, create_task_channels, start_server};
use llm_api::utils::cache_maintenance::start_maintenance_task;
use llm_api::utils::config::load_config;
use llm_api::utils::db::{create_db_pool, init_db, optimize_db};
use llm_api::utils::http_client::create_http_client;
use std::sync::Arc;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() {
    // 加载配置
    let config = load_config().expect("无法加载配置");
    
    println!(
        "服务配置: 数据库={}, 使用curl={}, 最大并发请求={}",
        config.database_url, config.use_curl, config.max_concurrent_requests
    );

    // 初始化数据库
    let pool = create_db_pool(&config.database_url)
        .await
        .expect("无法打开数据库");

    init_db(&pool).await.expect("数据库初始化失败");
    optimize_db(&pool).await.expect("数据库优化失败");
    
    // 启动缓存维护任务
    let pool_arc = Arc::new(pool.clone());
    start_maintenance_task(pool_arc, config.cache_maintenance.clone());
    
    // 创建任务处理通道和运行时
    let (tx_hit, tx_miss, _hit_runtime, _miss_runtime) = create_task_channels(
        config.cache_hit_pool_size,
        config.cache_miss_pool_size,
    );

    println!("创建HTTP客户端...");
    // 创建HTTP客户端
    let http_client = create_http_client().expect("无法创建HTTP客户端");

    // 创建应用状态
    let shared_state = Arc::new(AppState {
        db: Arc::new(pool),
        client: http_client,
        api_endpoints: config.api_endpoints.clone(),
        max_concurrent_requests: config.max_concurrent_requests,
        semaphore: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        cache_override_mode: config.cache_override_mode,
        use_curl: config.use_curl,
        use_proxy: config.use_proxy,
        api_headers: config.api_headers,
    });

    let app_state = Arc::new((shared_state.clone(), tx_hit, tx_miss));

    // 创建路由
    let app = create_router(app_state);

    // 启动服务器
    if let Err(e) = start_server(app).await {
        eprintln!("服务器启动失败: {}", e);
    }
}
