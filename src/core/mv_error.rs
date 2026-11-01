//! MV 播放器错误类型（内嵌自 rs-wgpu-video-player/player-core）。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum MvPlayerError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    #[error("no video stream in source")]
    NoVideoStream,
    #[error("no decoder found for codec")]
    DecoderNotFound,
    #[error("playback already running")]
    AlreadyRunning,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type MvResult<T> = std::result::Result<T, MvPlayerError>;
