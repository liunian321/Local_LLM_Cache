// 引入 prost 生成的 proto 模块
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/api.rs"));
}
pub mod models {
    pub mod api_model;
}

pub mod handlers {
    pub mod api_handler;
    pub mod chat_completion_handler;
    pub mod proxy_handler;
}
