use std::fmt::Display;

// 统一带请求ID的日志输出
pub fn log_with_id<T: Display>(id: &str, message: T) {
    println!("[{}] {}", id, message);
}

