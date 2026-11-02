//! 音频播放平台门面：定义统一契约，按 feature 选择平台实现并 re-export 同一组 API。
//! 调用方（core/network/load/handlers）永远只用 mod 导出的这组 free functions，
//! 不关心底层是 desktop(rodio/cpal)、web(WebAudio/HTMLAudio) 还是 mobile(系统媒体)。

/// 平台音频后端统一契约。
/// 各平台实现必须提供下述签名一致的 free functions（业务按此调用，永不改签名）：
///   play_at(bytes, total_sec, offset_sec)
///   stop / toggle / status / active / finished
///   begin_switch / end_switch / switching
///   set_volume / get_volume
///   list_devices / set_output_device / current_device_name
pub trait AudioBackend {}

#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
pub use desktop::*;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
pub use web::*;

// 后续（#33 mobile）：
// #[cfg(feature = "mobile")] mod mobile;  pub use mobile::*;
