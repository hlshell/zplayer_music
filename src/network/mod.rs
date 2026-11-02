//! 网络层：网易云客户端、音源、服务编排、代理/realip。
pub mod netease;
pub mod proxy;
pub mod service;
pub mod sources;
// 在线更新：桌面专属（web 不支持；移动端待定，未启用）
#[cfg(feature = "desktop")]
pub mod update;
