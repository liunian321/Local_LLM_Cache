use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;
use tokio::time;

use llm_api::utils::idle_flush::{IdleFlushConfig, IdleFlushManager};
use llm_api::utils::memory_cache::MemoryCache;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 创建或连接到SQLite数据库
    let db_path = "./idle_flush_test.db";

    // 创建数据库表
    let db = create_or_connect_db(db_path).await?;

    // 创建内存缓存
    let cache = Arc::new(MemoryCache::new(100));

    // 配置空闲刷新功能
    let idle_config = IdleFlushConfig {
        enabled: false,
        idle_timeout: Duration::from_secs(300),
        check_interval: Duration::from_secs(60),
    };

    // 创建空闲刷新管理器并配置数据库
    let flush_manager = Arc::new(
        IdleFlushManager::new(Arc::clone(&cache), idle_config).with_db(Arc::new(db), 1), // 缓存版本设为1
    );

    // 启动空闲刷新任务
    flush_manager.clone().start_flush_task().await;

    println!("模拟添加50个缓存项...");

    // 模拟添加一些缓存数据
    for i in 0..50 {
        let key = format!("key_{}", i);
        let value = format!("value_{}", i).into_bytes();
        cache.insert(key, value).await;

        // 更新活动时间
        flush_manager.update_activity().await;
    }

    println!("已添加50个缓存项");
    println!("缓存数量: {}", cache.cache_count());
    println!("待写入数量: {}", cache.pending_count());
    println!("等待30秒后触发空闲刷新...");

    // 等待一段时间，让空闲刷新功能触发
    time::sleep(Duration::from_secs(35)).await;

    // 检查结果
    println!("空闲刷新后：");
    println!("缓存数量: {}", cache.cache_count());
    println!("待写入数量: {}", cache.pending_count());

    // 再等待一段时间，确保程序不会立即退出
    time::sleep(Duration::from_secs(5)).await;

    Ok(())
}

// 创建或连接到SQLite数据库并初始化表结构
async fn create_or_connect_db(db_path: &str) -> Result<SqlitePool, sqlx::Error> {
    // 连接到数据库(如果不存在则创建)
    let db = SqlitePool::connect(&format!("sqlite:{}", db_path)).await?;

    // 创建答案表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS answers (
            key TEXT PRIMARY KEY,
            response BLOB NOT NULL,
            size INTEGER NOT NULL,
            hit_count INTEGER NOT NULL DEFAULT 0,
            version INTEGER NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&db)
    .await?;

    // 创建问题表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS questions (
            key TEXT PRIMARY KEY,
            answer_key TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (answer_key) REFERENCES answers(key)
        )",
    )
    .execute(&db)
    .await?;

    println!("数据库初始化完成: {}", db_path);

    Ok(db)
}
