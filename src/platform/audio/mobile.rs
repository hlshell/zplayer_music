//! 移动端音频后端：复用共享 rodio 核心，始终输出到系统默认设备。
//! 移动端不需要输出设备枚举/切换（系统媒体路由由 OS 处理），
//! 因此三个设备相关函数返回空/失败，保持与门面契约签名一致。

// 门面 API：阶段 1 移动切片尚无业务调用者，待阶段 3 接入播放后启用
#[allow(unused_imports)]
pub use super::common_rodio::{
    active, begin_switch, clear_failure, end_switch, finished, get_volume, paused, play_at,
    play_stream, set_volume, status, stop, switching, take_failure, toggle,
};

/// 移动端不提供输出设备枚举。
pub fn list_devices() -> Vec<String> {
    Vec::new()
}

/// 移动端不支持运行时切换输出设备。
pub fn set_output_device(_name: &str) -> bool {
    false
}

/// 移动端无目标设备名。
pub fn current_device_name() -> String {
    String::new()
}
