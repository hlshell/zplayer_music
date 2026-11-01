//! 核心领域逻辑与基础设施：歌曲模型/格式化、歌词解析、歌曲字节下载、播放队列、日志。
//! 均无 UI/平台依赖，供各层复用。
pub mod fetch;
pub mod ids;
pub mod logger;
pub mod lyrics;
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
