//! MV 播放器主结构体：封装 start/stop/seek/pause，暴露给 UI handler 调用。
//! 内嵌自 rs-wgpu-video-player/player-core/src/lib.rs；音频走 cpal（有音频流时视频跟音频时钟）。

use crate::core::mv_config::MvPlaybackConfig;
use crate::core::mv_decoder::mv_loop_decoder;
use crate::core::mv_error::MvPlayerError;
use crate::core::mv_event::MvPlaybackEvent;
use crate::core::mv_frame::MvFrameBuffer;
use crate::core::mv_state::MvPlaybackState;

use std::sync::{Arc, mpsc};
use tokio::sync::mpsc::UnboundedReceiver;

/// MV 播放器（音频 cpal 输出，视频帧跟音频时钟 pacing）。
pub struct MvPlayer {
    pub frame_buffer: MvFrameBuffer,
    is_initialized: bool,
    state: Arc<MvPlaybackState>,
    cfg: MvPlaybackConfig,
    /// 运行中播放会话的命令通道（drop 即 demux loop 退出）。
    cmd_tx: Option<mpsc::Sender<crate::core::mv_decoder::MvDemuxCommand>>,
}

impl MvPlayer {
    pub fn new() -> Self {
        Self::with_config(MvPlaybackConfig::default())
    }

    pub fn with_config(cfg: MvPlaybackConfig) -> Self {
        Self {
            frame_buffer: MvFrameBuffer::new(),
            is_initialized: false,
            state: Arc::new(MvPlaybackState::new()),
            cfg,
            cmd_tx: None,
        }
    }

    pub fn state(&self) -> Arc<MvPlaybackState> {
        self.state.clone()
    }

    pub fn start_playback(&mut self, url: &str)
        -> Result<UnboundedReceiver<MvPlaybackEvent>, MvPlayerError>
    {
        if self.is_initialized {
            return Err(MvPlayerError::AlreadyRunning);
        }
        self.state.reset();
        self.is_initialized = true;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<MvPlaybackEvent>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<crate::core::mv_decoder::MvDemuxCommand>();
        self.cmd_tx = Some(cmd_tx);

        let url = url.to_string();
        let buffer = self.frame_buffer.clone();
        let state = self.state.clone();
        let cfg = self.cfg;
        // 注意：代理启动（含源站大小探测、监听绑定）必须放在后台线程，
        // 绝不能在 start_playback（UI 事件回调线程）里同步执行——网络慢时会
        // 阻塞 UI 线程数秒，表现为"点 MV 没反应"。代理生命周期跟解码任务绑定。
        tokio::task::spawn_blocking(move || {
            // http(s) 源走本地加速代理：后端 4 线程 Range 并行回源；
            // 优先复用预缓冲的下一首代理（已在后台下载分片），未命中则启动新代理。
            // 探测/启动失败（源站不支持 Range、超时等）静默回退直连。
            let mut dec_url = url.clone();
            let _proxy = if url.starts_with("http://") || url.starts_with("https://") {
                crate::core::mv_fetch::take_pre_buffered(&url)
                    .or_else(|| crate::core::mv_fetch::MvAccelProxy::start(&url))
            } else {
                None
            };
            if let Some(p) = &_proxy {
                dec_url = p.local_url();
            }
            if let Err(e) = mv_loop_decoder(dec_url, buffer, tx.clone(), state, cfg, cmd_rx) {
                let _ = tx.send(MvPlaybackEvent::Error(format!("{e}")));
            }
            // 任务退出（正常播完 / shutdown / 切歌）时 _proxy drop，关闭 worker 与监听
        });
        Ok(rx)
    }

    pub fn stop_playback(&mut self) {
        if !self.is_initialized { return; }
        self.cmd_tx = None;
        self.state.request_shutdown();
        self.is_initialized = false;
    }

    pub fn seek_to_us(&self, us: u64) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(crate::core::mv_decoder::MvDemuxCommand::Seek(us));
        }
    }

    pub fn toggle_pause(&self) -> bool {
        self.state.toggle_pause()
    }

    pub fn set_volume(&self, vol: u32) {
        self.state.set_volume(vol);
    }
}

impl Default for MvPlayer {
    fn default() -> Self { Self::new() }
}

impl Drop for MvPlayer {
    fn drop(&mut self) {
        self.state.request_shutdown();
    }
}
