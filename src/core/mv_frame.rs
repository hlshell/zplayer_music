//! MV 播放器帧缓冲（内嵌自 rs-wgpu-video-player/player-core）。
//! 使用 tokio::sync::watch 做 latest-frame-wins 管道——解码器帧率可能 60fps，
//! 但 UI 只消费 ~60fps，覆盖模型天然丢旧帧，不会积压内存。

use std::sync::Arc;
use tokio::sync::watch::{channel, Receiver, Sender};

#[derive(Debug, Clone)]
pub struct MvFrame {
    pub width: u32,
    pub height: u32,
    /// RGBA8 / BGRA8 像素缓冲。
    pub data: Arc<[u8]>,
    /// PTS（微秒），解码时按容器 time_base 换算。
    pub ts_us: u64,
}

/// Latest-frame-wins 通道。`push` 覆盖当前；订阅方永远只看到最新帧。
#[derive(Clone)]
pub struct MvFrameBuffer {
    tx: Sender<Option<Arc<MvFrame>>>,
    rx: Receiver<Option<Arc<MvFrame>>>,
}

impl Default for MvFrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl MvFrameBuffer {
    pub fn new() -> Self {
        let (tx, rx) = channel(None);
        Self { tx, rx }
    }
    pub fn push(&self, frame: Arc<MvFrame>) {
        let _ = self.tx.send(Some(frame));
    }
    pub fn subscribe(&self) -> Receiver<Option<Arc<MvFrame>>> {
        self.rx.clone()
    }
}
