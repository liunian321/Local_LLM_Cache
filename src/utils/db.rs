use sqlx::{Executor, SqlitePool};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

// 初始化数据库和表结构
pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // 创建答案表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS answers (
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

    // 创建问题表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS questions (
            key TEXT PRIMARY KEY,
            answer_key TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            FOREIGN KEY(answer_key) REFERENCES answers(key)
        )",
    )
    .execute(pool)
    .await?;

    // 创建索引以提高查询速度
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_answers_key ON answers(key)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_answers_version ON answers(version)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_questions_key ON questions(key)")
        .execute(pool)
        .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_questions_answer_key ON questions(answer_key)")
        .execute(pool)
        .await?;

    // 如果存在旧的cache表，迁移数据到新表
    let exists_cache = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='cache'"
    )
    .fetch_optional(pool)
    .await?;

    if exists_cache.is_some() {
        println!("检测到旧的cache表，开始数据迁移...");
        
        // 从cache表中复制数据到answers表和questions表
        sqlx::query(
            "INSERT OR IGNORE INTO answers (key, response, size, hit_count, version)
             SELECT key, response, size, hit_count, version FROM cache"
        )
        .execute(pool)
        .await?;

        sqlx::query(
            "INSERT OR IGNORE INTO questions (key, answer_key)
             SELECT key, key FROM cache"
        )
        .execute(pool)
        .await?;

        println!("数据迁移完成");
        
        // 重命名旧表而不是删除，以保留数据
        println!("重命名旧的cache表为cache_backup...");
        sqlx::query("ALTER TABLE cache RENAME TO cache_backup")
            .execute(pool)
            .await?;
        println!("旧表已重命名为cache_backup");
    }

    Ok(())
}

pub async fn optimize_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
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

// 创建数据库连接池
pub async fn create_db_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(100)
        .min_connections(10) // 增加最小连接数，降低连接启动开销
        .max_lifetime(std::time::Duration::from_secs(1800)) // 连接最长生命周期30分钟
        .idle_timeout(std::time::Duration::from_secs(600)) // 闲置超时10分钟
        .connect_with(
            SqliteConnectOptions::new()
                .filename(database_url)
                .create_if_missing(true)
                .foreign_keys(false) // 禁用外键约束检查以提高性能
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal) // 使用WAL模式
                .synchronous(sqlx::sqlite::SqliteSynchronous::Normal), // 降低同步级别
        )
        .await
} 