use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use llm_api::models::api_model::AppState;
use llm_api::server::{create_router, start_server};
use llm_api::utils::config::load_config;
use llm_api::utils::db::{create_db_pool, init_db, optimize_db};
use llm_api::utils::http_client::create_http_client;
use llm_api::utils::memory_cache::MemoryCache;
use llm_api::utils::cache_maintenance::start_maintenance_task;
use llm_api::utils::idle_flush::{IdleFlushConfig, IdleFlushManager};

#[tokio::main]
async fn main() {
    // 加载配置
    let config = match load_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("加载配置失败: {}", e);
            return;
        }
    };

    // 创建数据库连接池
    let pool = match create_db_pool(&config.database_url).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("创建数据库连接池失败: {}", e);
            return;
        }
    };
    
    // 初始化数据库
    if let Err(e) = init_db(&pool).await {
        eprintln!("初始化数据库失败: {}", e);
        return;
    }
    
    // 优化数据库
    if let Err(e) = optimize_db(&pool).await {
        eprintln!("优化数据库失败: {}", e);
        return;
    }

    // 创建HTTP客户端
    let http_client = match create_http_client() {
        Ok(client) => client,
        Err(e) => {
            eprintln!("创建HTTP客户端失败: {}", e);
            return;
        }
    };

    // 创建缓存命中和未命中的任务发送器
    let (tx_hit, _) = mpsc::channel(config.cache_hit_pool_size);
    let (tx_miss, _) = mpsc::channel(config.cache_miss_pool_size);

    // 初始化内存缓存
    let memory_cache = if config.cache.enabled {
        println!("初始化内存缓存，最大容量: {} 条", config.cache.max_items);
        Some(Arc::new(MemoryCache::new(config.cache.max_items)))
    } else {
        println!("内存缓存功能已禁用");
        None
    };

    // 创建应用状态
    let shared_state = Arc::new(AppState {
        db: Arc::new(pool.clone()),
        client: http_client,
        api_endpoints: config.api_endpoints.clone(),
        max_concurrent_requests: config.max_concurrent_requests,
        semaphore: Arc::new(Semaphore::new(config.max_concurrent_requests)),
        cache_override_mode: config.cache_override_mode,
        use_curl: config.use_curl,
        use_proxy: config.use_proxy,
        enable_thinking: config.enable_thinking,
        api_headers: config.api_headers,
        memory_cache: memory_cache.clone(),
        cache_enabled: config.cache.enabled,
        batch_write_size: config.cache.batch_write_size,
    });

    // 启动缓存维护任务
    if config.cache_maintenance.enabled {
        println!("启动缓存维护任务");
        start_maintenance_task(
            Arc::new(pool.clone()),
            config.cache_maintenance.clone(),
        );
    }

    // 启动空闲刷新任务
    if config.idle_flush.enabled && memory_cache.is_some() {
        println!("启动空闲刷新任务");
        let idle_config = IdleFlushConfig::from_yaml_config(&config.idle_flush);
        
        let idle_manager = Arc::new(
            IdleFlushManager::new(memory_cache.clone().unwrap(), idle_config)
                .with_db(Arc::new(pool.clone()), 1) // 使用当前缓存版本
        );
        
        idle_manager.clone().start_flush_task().await;
        println!("空闲刷新任务已启动");
    }

    let app_state = Arc::new((shared_state.clone(), tx_hit, tx_miss));

    // 创建路由
    let app = create_router(app_state);

    // 启动服务器
    if let Err(e) = start_server(app).await {
        eprintln!("服务器启动失败: {}", e);
    }
}
