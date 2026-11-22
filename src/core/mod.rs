//! 核心领域逻辑与基础设施：歌曲模型/格式化、歌词解析、歌曲字节下载、播放队列、日志。
//! 均无 UI/平台依赖，供各层复用。
pub mod fetch;
pub mod ids;
#[cfg(feature = "desktop")]
pub mod logger;
// 阶段 3 临时切片：移动端无文件日志与崩溃钩子（依赖 simplelog），仅保留可编译空实现
#[cfg(feature = "mobile")]
pub mod logger {
    pub fn init() {}
    pub fn mark_clean_exit() {}
}
// 歌词：parse_lrc / LYRICS 为纯逻辑，移动端复用；模型构造函数依赖桌面 UI 类型，模块内门控
#[cfg(any(feature = "desktop", feature = "mobile"))]
pub mod lyrics;
// 队列逻辑直接操作桌面端 AppWindow 的队列/当前歌曲属性，移动端 UI 无对应属性
#[cfg(feature = "desktop")]
pub mod queue;
pub mod song_util;

// MV 播放器（内嵌自 rs-wgpu-video-player/player-core；仅 desktop）
#[cfg(feature = "desktop")]
pub mod mv_audio;
#[cfg(feature = "desktop")]
pub mod mv_config;
#[cfg(feature = "desktop")]
pub mod mv_decoder;
#[cfg(feature = "desktop")]
pub mod mv_decoder_audio;
#[cfg(feature = "desktop")]
pub mod mv_decoder_hw;
#[cfg(feature = "desktop")]
pub mod mv_decoder_video;
#[cfg(feature = "desktop")]
pub mod mv_demux;
#[cfg(feature = "desktop")]
pub mod mv_error;
#[cfg(feature = "desktop")]
pub mod mv_event;
#[cfg(feature = "desktop")]
pub mod mv_fetch;
#[cfg(feature = "desktop")]
pub mod mv_frame;
#[cfg(feature = "desktop")]
pub mod mv_frame_convert;
#[cfg(feature = "desktop")]
pub mod mv_player;
#[cfg(feature = "desktop")]
pub mod mv_state;
