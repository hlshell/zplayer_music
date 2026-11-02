//! MV 播放器播放管线调优（内嵌自 rs-wgpu-video-player/player-core）。
//! 默认值直接内嵌，和原工程保持一致；加了 target_width/height 供缩放显示。

use std::time::Duration;

/// 解码器输出的 4 字节像素布局（对齐 Slint SharedPixelBuffer<Rgba8Pixel>）。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MvPixelFormat {
    /// R, G, B, A
    Rgba,
}

#[derive(Copy, Clone, Debug)]
pub struct MvPlaybackConfig {
    /// Demux → 视频解码的包队列容量。
    pub video_pkt_queue: usize,
    /// Demux → 音频解码的包队列容量。
    pub audio_pkt_queue: usize,
    /// 视频帧晚于 PTS 多少微秒直接丢弃。
    pub late_drop_us: i64,
    /// 视频 pacing 单次 sleep 切片上限（保持 pause/shutdown 响应）。
    pub pace_slice: Duration,
    /// 帧缓冲里像素布局。
    pub output_format: MvPixelFormat,
    /// 可选：sws_scale 直接缩到目标显示宽度（0 = 保持源分辨率）。
    /// MV 播放器固定用 0：清晰度只由所选流决定（480p/720p/1080p 源各自原分辨率），
    /// 统一缩到 720p 会让三档看起来完全一样（旧的"清晰度没变化"根因）。
    pub target_width: u32,
    pub target_height: u32,
    /// 缓冲门控进入阈值（微秒）：前方可用数据低于此值即暂停出画+静音，UI 显示"正在缓冲"。
    /// 0 = 关闭门控。
    pub buffer_enter_us: u64,
    /// 缓冲门控恢复阈值（微秒）：缓冲到该时长以上才恢复播放（避免一边缓冲一边卡顿）。
    pub buffer_resume_us: u64,
    /// 判定"已读到流尾"的余量（微秒）：读入水位距总时长小于该值即不做门控。
    pub buffer_tail_us: u64,
}

impl Default for MvPlaybackConfig {
    fn default() -> Self {
        Self {
            video_pkt_queue: 4096,
            // 音频包队列 ≈190s（AAC 每帧约 23ms × 8192），与视频队列（≈163s）对齐。
            // 原来 1024 包只有 ~24 秒预缓冲：demux 按音视频交错顺序读流，音频队列先满
            // 即背压停读，整个预缓冲窗口被锁死在 24 秒——表现为"缓冲突然停止，
            // 播放到缓冲点才重新请求网络"。内存代价约 3-7MB，可接受。
            audio_pkt_queue: 8192,
            late_drop_us: 250_000,        // 放宽到 250ms：系统调度抖动时不轻易丢帧
            pace_slice: Duration::from_millis(10),  // 更细的 pacing 切片，画面更顺滑
            output_format: MvPixelFormat::Rgba,
            target_width: 0,
            target_height: 0,
            // 缓冲门控：低于 2s 才暂停（0.8s 太敏感，网络轻微波动就卡顿）；
            // 恢复阈值 4s（原 5s 等待偏长），减少"卡在缓冲"的体感
            buffer_enter_us: 2_000_000,
            buffer_resume_us: 4_000_000,
            buffer_tail_us: 1_000_000,
        }
    }
}
