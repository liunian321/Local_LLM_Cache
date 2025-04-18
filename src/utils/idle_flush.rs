use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::time;
use sqlx::SqlitePool;

use crate::utils::memory_cache::MemoryCache;
use crate::utils::db_writer::DbWriter;

pub struct IdleFlushConfig {
    pub enabled: bool,
    pub idle_timeout: Duration,
    pub check_interval: Duration,
}

impl Default for IdleFlushConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            idle_timeout: Duration::from_secs(300), // 默认5分钟
            check_interval: Duration::from_secs(60), // 默认1分钟检查一次
        }
    }
}

impl IdleFlushConfig {
    /// 从配置文件数据创建配置
    pub fn from_yaml_config(idle_flush_config: &crate::utils::config::IdleFlushConfig) -> Self {
        Self {
            enabled: idle_flush_config.enabled,
            idle_timeout: Duration::from_secs(idle_flush_config.idle_timeout_seconds),
            check_interval: Duration::from_secs(idle_flush_config.check_interval_seconds),
        }
    }
}

pub struct IdleFlushManager {
    cache: Arc<MemoryCache>,
    config: IdleFlushConfig,
    last_activity: Mutex<Instant>,
    db_writer: Option<DbWriter>,
}

impl IdleFlushManager {
    pub fn new(cache: Arc<MemoryCache>, config: IdleFlushConfig) -> Self {
        Self {
            cache,
            config,
            last_activity: Mutex::new(Instant::now()),
            db_writer: None,
        }
    }

    pub fn with_db(mut self, db: Arc<SqlitePool>, cache_version: u8) -> Self {
        self.db_writer = Some(DbWriter::new(db, cache_version));
        self
    }

    pub async fn update_activity(&self) {
        let mut last_activity = self.last_activity.lock().await;
        *last_activity = Instant::now();
    }

    pub async fn start_flush_task(self: Arc<Self>) {
        if !self.config.enabled {
            println!("空闲刷新功能已禁用");
            return;
        }

        println!(
            "启动空闲刷新任务：空闲超时 {:?}，检查间隔 {:?}",
            self.config.idle_timeout,
            self.config.check_interval
        );

        tokio::spawn(async move {
            let check_interval = self.config.check_interval;
            
            loop {
                time::sleep(check_interval).await;
                
                let is_idle = {
                    let last_activity = self.last_activity.lock().await;
                    last_activity.elapsed() >= self.config.idle_timeout
                };
                
                if is_idle {
                    // 空闲时间已达到，刷新所有缓存
                    let pending_count = self.cache.pending_count();
                    let cache_count = self.cache.cache_count();
                    
                    if pending_count > 0 || cache_count > 0 {
                        println!("系统空闲超过 {:?}，开始刷新缓存", self.config.idle_timeout);
                        println!("当前缓存项数量: {}, 待写入项数量: {}", cache_count, pending_count);
                        
                        // 将所有待写入的项取出
                        let pending_items = self.cache.take_pending_writes(pending_count);
                        
                        // 将当前缓存中的所有项移到待写入状态并取出
                        let cache_items = self.cache.flush_all_to_pending().await;
                        
                        // 合并所有需要写入的项
                        let mut all_items = Vec::with_capacity(pending_items.len() + cache_items.len());
                        all_items.extend(pending_items);
                        all_items.extend(cache_items);
                        
                        // 如果有数据库写入工具，执行写入操作
                        if let Some(writer) = &self.db_writer {
                            let total_items = all_items.len();
                            if total_items > 0 {
                                println!("空闲刷新: 开始将 {} 个缓存项写入数据库", total_items);
                                let (success, failed) = writer.batch_write(all_items).await;
                                println!("空闲刷新: 数据库写入完成，成功: {}，失败: {}", success, failed);
                            }
                        } else {
                            println!("空闲刷新: 未配置数据库连接，跳过写入操作");
                        }
                        
                        // 重置活动时间
                        self.update_activity().await;
                    }
                }
            }
        });
    }
} 