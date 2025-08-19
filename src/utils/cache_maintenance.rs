use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CacheMaintenanceConfig {
    pub enabled: bool,
    pub interval_hours: u64,
    pub retention_days: i64,
    pub cleanup_on_startup: bool,
    pub min_hit_count: i64,
}

impl Default for CacheMaintenanceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: 12,
            retention_days: 30,
            cleanup_on_startup: false,
            min_hit_count: 5,
        }
    }
}

// 打印缓存统计信息
pub async fn print_cache_stats(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // 查询问题表的统计信息
    let questions_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM questions")
        .fetch_one(pool)
        .await?;

    // 查询答案表的统计信息
    let answers_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM answers")
        .fetch_one(pool)
        .await?;

    // 查询重复利用率 (问题数量 / 答案数量)
    let reuse_ratio = if answers_count > 0 {
        questions_count as f64 / answers_count as f64
    } else {
        0.0
    };

    // 查询总缓存大小
    let total_size = sqlx::query_scalar::<_, i64>("SELECT SUM(size) FROM answers")
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);

    // 查询命中率高的答案
    let top_hits = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT key, hit_count, size FROM answers ORDER BY hit_count DESC LIMIT 5",
    )
    .fetch_all(pool)
    .await?;

    println!("=== 缓存统计信息 ===");
    println!("问题数量: {}", questions_count);
    println!("答案数量: {}", answers_count);
    println!("问答复用率: {:.2}", reuse_ratio);
    println!(
        "总缓存大小: {} 字节 ({:.2} MB)",
        total_size,
        total_size as f64 / (1024.0 * 1024.0)
    );

    if !top_hits.is_empty() {
        println!("命中率最高的答案:");
        for (key, hits, size) in top_hits {
            println!(
                "  Key: {}... | 命中次数: {} | 大小: {} 字节",
                key.chars().take(8).collect::<String>(),
                hits,
                size
            );
        }
    }

    Ok(())
}

// 清理旧的备份表
pub async fn cleanup_backup_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // 检查是否存在备份表
    let exists_backup = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='cache_backup'",
    )
    .fetch_optional(pool)
    .await?;

    if exists_backup.is_some() {
        println!("发现备份表cache_backup，正在删除...");
        sqlx::query("DROP TABLE cache_backup").execute(pool).await?;
        println!("备份表cache_backup已删除");
    }

    Ok(())
}

// 清理过期缓存
pub async fn cleanup_old_entries(
    pool: &SqlitePool,
    days: i64,
    min_hit_count: i64,
) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now().timestamp();
    let cutoff = now - days * 24 * 60 * 60; // 转换天数为秒

    // 开始事务
    let mut tx = pool.begin().await?;

    // 首先找出将要删除的答案
    let orphaned_answers = sqlx::query_scalar::<_, String>(
        "SELECT a.key FROM answers a 
         LEFT JOIN questions q ON a.key = q.answer_key 
         WHERE q.key IS NULL AND a.hit_count < ? AND a.created_at < ?",
    )
    .bind(min_hit_count)
    .bind(cutoff)
    .fetch_all(&mut *tx)
    .await?;

    let answers_count = orphaned_answers.len();

    if answers_count > 0 {
        // 删除过期且无引用的答案
        let deleted = sqlx::query(
            "DELETE FROM answers 
             WHERE key IN (
                SELECT a.key FROM answers a 
                LEFT JOIN questions q ON a.key = q.answer_key 
                WHERE q.key IS NULL AND a.hit_count < ? AND a.created_at < ?
             )",
        )
        .bind(min_hit_count)
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;

        println!("已清理 {} 条过期答案记录", deleted.rows_affected());
    }

    // 删除过期的问题（但保留引用的答案）
    let deleted_questions = sqlx::query("DELETE FROM questions WHERE created_at < ?")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;

    println!(
        "已清理 {} 条过期问题记录",
        deleted_questions.rows_affected()
    );

    // 提交事务
    tx.commit().await?;

    // 打印缓存统计
    print_cache_stats(pool).await?;

    Ok(())
}

// 启动后台缓存维护任务
pub fn start_maintenance_task(pool: Arc<SqlitePool>, config: CacheMaintenanceConfig) {
    if !config.enabled {
        println!("缓存维护功能已禁用");
        return;
    }

    // 如果配置为启动时执行清理，则立即执行一次
    if config.cleanup_on_startup {
        let pool_clone = pool.clone();
        let min_hit_count = config.min_hit_count;
        let retention_days = config.retention_days;

        tokio::spawn(async move {
            println!("执行启动时缓存清理...");
            if let Err(e) = cleanup_old_entries(&pool_clone, retention_days, min_hit_count).await {
                eprintln!("启动时缓存清理失败: {}", e);
            }
        });
    }

    // 后台任务：定期清理和统计
    let interval_hours = config.interval_hours;
    let retention_days = config.retention_days;
    let min_hit_count = config.min_hit_count;

    tokio::spawn(async move {
        // 等待5秒，避免与启动清理同时执行
        tokio::time::sleep(Duration::from_secs(5)).await;

        // 先清理备份表
        tokio::time::sleep(Duration::from_secs(3600)).await; // 等待1小时
        if let Err(e) = cleanup_backup_table(&pool).await {
            eprintln!("清理备份表失败: {}", e);
        }

        // 定期执行清理任务
        let interval = Duration::from_secs(interval_hours * 60 * 60);
        let mut interval_timer = tokio::time::interval(interval);

        println!("缓存维护任务已启动，间隔: {}小时", interval_hours);

        loop {
            interval_timer.tick().await;

            println!("执行定期缓存维护...");
            if let Err(e) = cleanup_old_entries(&pool, retention_days, min_hit_count).await {
                eprintln!("缓存维护失败: {}", e);
            } else {
                println!("缓存维护完成");
            }
        }
    });
}
