//! MV 播放器事件（内嵌自 rs-wgpu-video-player/player-core）。

#[derive(Debug, Clone)]
pub enum MvPlaybackEvent {
    /// 容器总时长（微秒）。
    Duration(u64),
    /// 播放到结尾。
    Ended,
    /// 播放出错（字符串消息）。
    Error(String),
}
