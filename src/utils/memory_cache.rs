use dashmap::DashMap;
use tokio::sync::Mutex;
use std::collections::VecDeque;

pub struct MemoryCache {
    cache: DashMap<String, Vec<u8>>,
    queue: Mutex<VecDeque<String>>,
    max_items: usize,
    pending_writes: DashMap<String, Vec<u8>>,
}

impl MemoryCache {
    pub fn new(max_items: usize) -> Self {
        Self {
            cache: DashMap::new(),
            queue: Mutex::new(VecDeque::with_capacity(max_items)),
            max_items,
            pending_writes: DashMap::new(),
        }
    }

    // 获取缓存项
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.cache.get(key).map(|value| value.clone())
    }

    // 添加缓存项
    pub async fn insert(&self, key: String, value: Vec<u8>) {
        // 如果已经存在，只更新值
        if self.cache.contains_key(&key) {
            self.cache.insert(key, value);
            return;
        }

        // 获取锁进行队列操作
        let mut queue = self.queue.lock().await;
        
        // 如果达到容量上限，需要移除最早的项
        if queue.len() >= self.max_items {
            if let Some(oldest_key) = queue.pop_front() {
                // 将被移除的项放入待写入队列
                if let Some((_, value)) = self.cache.remove(&oldest_key) {
                    self.pending_writes.insert(oldest_key, value);
                }
            }
        }
        
        // 插入新项
        queue.push_back(key.clone());
        self.cache.insert(key, value);
    }

    // 获取待写入的项
    pub fn take_pending_writes(&self, batch_size: usize) -> Vec<(String, Vec<u8>)> {
        let mut result = Vec::with_capacity(batch_size);
        let mut count = 0;
        
        // 获取并移除指定数量的待写入项
        let pending_keys: Vec<String> = self.pending_writes.iter()
            .take(batch_size)
            .map(|entry| entry.key().clone())
            .collect();
        
        for key in pending_keys {
            if let Some((k, v)) = self.pending_writes.remove(&key) {
                result.push((k, v));
                count += 1;
                if count >= batch_size {
                    break;
                }
            }
        }
        
        result
    }

    // 将所有缓存项移动到待写入状态并返回这些项
    pub async fn flush_all_to_pending(&self) -> Vec<(String, Vec<u8>)> {
        // 获取所有缓存键
        let cache_keys: Vec<String> = self.cache.iter()
            .map(|entry| entry.key().clone())
            .collect();
        
        let mut result = Vec::with_capacity(cache_keys.len());
        
        // 清空队列
        let mut queue = self.queue.lock().await;
        queue.clear();
        
        // 将所有缓存项移到待写入状态
        for key in cache_keys {
            if let Some((k, v)) = self.cache.remove(&key) {
                self.pending_writes.insert(k.clone(), v.clone());
                result.push((k, v));
            }
        }
        
        result
    }

    // 获取待写入项数量
    pub fn pending_count(&self) -> usize {
        self.pending_writes.len()
    }

    // 获取当前缓存项数量
    pub fn cache_count(&self) -> usize {
        self.cache.len()
    }
} 