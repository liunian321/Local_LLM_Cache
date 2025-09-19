use reqwest;
use std::time::Duration;
use crate::utils::config::HttpClientConfig;

pub fn create_http_client(config: &HttpClientConfig) -> Result<reqwest::Client, reqwest::Error> {
    // HTTP客户端配置
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_seconds))
        .connect_timeout(Duration::from_secs(config.connect_timeout_seconds))
        .tcp_nodelay(true)
        .tcp_keepalive(Some(Duration::from_secs(config.tcp_keepalive_seconds)))
        .pool_idle_timeout(Duration::from_secs(config.pool_idle_timeout_seconds)) // 空闲连接超时
        .pool_max_idle_per_host(config.pool_max_idle_per_host) // 每个主机最大空闲连接数
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(config.max_redirects))
        .http1_title_case_headers()
        .http2_adaptive_window(true) // HTTP/2自适应窗口大小
        .http2_keep_alive_interval(Some(Duration::from_secs(config.http2_keep_alive_interval_seconds)))
        .http2_keep_alive_timeout(Duration::from_secs(config.http2_keep_alive_timeout_seconds))
        .http2_initial_stream_window_size(config.http2_initial_stream_window_size as u32) // 1MB窗口大小
        .no_proxy() // 禁用代理
        .build()
}
