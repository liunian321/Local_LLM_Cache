use reqwest;
use std::time::Duration;

pub fn create_http_client() -> Result<reqwest::Client, reqwest::Error> {
    // 优化HTTP客户端配置
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .tcp_keepalive(Some(Duration::from_secs(60)))
        .pool_idle_timeout(Duration::from_secs(180)) // 增加空闲连接超时
        .pool_max_idle_per_host(50) // 增加每个主机最大空闲连接数
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .http1_title_case_headers()
        .http2_adaptive_window(true) // 启用HTTP/2自适应窗口大小
        .http2_keep_alive_interval(Some(Duration::from_secs(30)))
        .http2_keep_alive_timeout(Duration::from_secs(30))
        .http2_initial_stream_window_size(1024 * 1024) // 1MB窗口大小
        .no_proxy() // 禁用代理
        .build()
} 