use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::sync::Arc;

/// 数据库写入工具，用于将缓存数据写入到数据库
pub struct DbWriter {
    db: Arc<SqlitePool>,
    cache_version: u8,
}

impl DbWriter {
    /// 创建新的数据库写入工具
    pub fn new(db: Arc<SqlitePool>, cache_version: u8) -> Self {
        Self { db, cache_version }
    }

    /// 批量写入数据到数据库
    pub async fn batch_write(&self, items: Vec<(String, Vec<u8>)>) -> (usize, usize) {
        let items_len = items.len();
        if items_len == 0 {
            return (0, 0);
        }

        println!("开始批量写入 {} 条缓存数据到数据库", items_len);

        // 使用事务进行批量写入
        let tx_result = self.db.begin().await;
        if let Err(e) = tx_result {
            eprintln!("开始数据库事务失败: {}", e);
            return (0, items_len);
        }

        let mut tx = tx_result.unwrap();
        let mut success_count = 0;

        for (question_key, compressed) in items {
            let data_size = compressed.len() as i64;

            // 计算答案的哈希作为key
            let mut hasher = Sha256::new();
            hasher.update(&compressed);
            let answer_key = hex::encode(hasher.finalize());

            // 1. 插入答案表
            let answer_result = sqlx::query(
                "INSERT OR IGNORE INTO answers (key, response, size, hit_count, version) 
                 VALUES (?, ?, ?, 0, ?)",
            )
            .bind(&answer_key)
            .bind(&compressed)
            .bind(data_size)
            .bind(self.cache_version)
            .execute(&mut *tx)
            .await;

            if let Err(e) = answer_result {
                eprintln!("批量写入: 插入答案记录失败: {}", e);
                continue;
            }

            // 2. 插入问题表
            let question_result = sqlx::query(
                "INSERT OR REPLACE INTO questions (key, answer_key) 
                 VALUES (?, ?)",
            )
            .bind(&question_key)
            .bind(&answer_key)
            .execute(&mut *tx)
            .await;

            if let Err(e) = question_result {
                eprintln!("批量写入: 插入问题记录失败: {}", e);
                continue;
            }

            success_count += 1;
        }

        // 提交事务
        if let Err(e) = tx.commit().await {
            eprintln!("批量写入: 提交事务失败: {}", e);
            return (success_count, items_len - success_count);
        }

        println!("批量写入完成，成功: {}/{}", success_count, items_len);
        (success_count, items_len - success_count)
    }

    /// 写入单个缓存项到数据库
    pub async fn write_single(&self, question_key: String, compressed: Vec<u8>) -> bool {
        let data_size = compressed.len() as i64;

        // 计算答案的哈希作为key
        let mut hasher = Sha256::new();
        hasher.update(&compressed);
        let answer_key = hex::encode(hasher.finalize());

        // 使用事务确保数据一致性
        let tx_result = self.db.begin().await;
        if let Err(e) = tx_result {
            eprintln!("开始数据库事务失败: {}", e);
            return false;
        }

        let mut tx = tx_result.unwrap();

        // 1. 插入或更新答案表
        let answer_result = sqlx::query(
            "INSERT OR IGNORE INTO answers (key, response, size, hit_count, version) 
             VALUES (?, ?, ?, 0, ?)",
        )
        .bind(&answer_key)
        .bind(&compressed)
        .bind(data_size)
        .bind(self.cache_version)
        .execute(&mut *tx)
        .await;

        if let Err(e) = answer_result {
            eprintln!("插入答案记录失败: {}", e);
            let _ = tx.rollback().await;
            return false;
        }

        // 2. 插入或更新问题表
        let question_result = sqlx::query(
            "INSERT OR REPLACE INTO questions (key, answer_key) 
             VALUES (?, ?)",
        )
        .bind(&question_key)
        .bind(&answer_key)
        .execute(&mut *tx)
        .await;

        if let Err(e) = question_result {
            eprintln!("插入问题记录失败: {}", e);
            let _ = tx.rollback().await;
            return false;
        }

        // 提交事务
        if let Err(e) = tx.commit().await {
            eprintln!("提交事务失败: {}", e);
            return false;
        }

        println!(
            "成功缓存响应 Size: {}, Answer Key: {}",
            data_size, answer_key
        );
        true
    }
}
