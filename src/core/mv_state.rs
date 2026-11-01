//! MV 播放器共享原子状态（内嵌自 rs-wgpu-video-player/player-core）。
//! 解码线程、cpal 音频回调、UI 轮询都读这些标志——集中管理避免到处加字段。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// seek 目标哨兵（尚未发生 seek）。
const NO_SEEK_US: u64 = u64::MAX;

#[derive(Default)]
pub struct MvPlaybackState {
    pub shutdown: AtomicBool,
    pub paused: AtomicBool,
    /// 音量 0-100。
    pub volume: AtomicU32,
    /// Seek 代数。Demux 每次 seek 自增；包带有自己的 epoch，解码线程丢弃过期包。
    pub epoch: AtomicU64,
    /// 会话首帧 PTS（微秒，绝对时间基准）。部分流（HLS/直播）首帧 PTS 非零，
    /// demux seek 时必须以它为偏移基准，否则目标位置会偏早（用户看到"后退"）。
    /// 值本身用 0 表示"尚未设置"，但首帧 PTS 恰好为 0 也是合法的——
    /// 因此用 first_pts_set 标志区分，避免会话中期被后续帧改写基准。
    pub first_pts_us: AtomicU64,
    /// 首帧基准是否已锁定。
    pub first_pts_set: AtomicBool,
    /// seek 目标（微秒，相对 first_pts；NO_SEEK_US = 无）。
    /// demux seek 后通常落在目标点之前的最近关键帧：视频/音频解码线程据此
    /// 快速丢弃早于目标点的帧与采样，否则用户点了进度条却从关键帧重新播放。
    pub seek_target_us: AtomicU64,
    /// 预加载水位（微秒，相对 first_pts）：demux 已读入包的最大时间点。
    /// 进度条据此绘制"已缓冲"区间。
    pub buffered_us: AtomicU64,
    /// 会话总时长（微秒，0 = 未知）。
    pub duration_us: AtomicU64,
    /// 缓冲门控：true = 前方可用数据不足，暂停音视频输出（UI 显示"正在缓冲"）。
    pub buffering: AtomicBool,
    /// demux 已读完整个流（尾部不再做缓冲门控，否则结尾几帧会永远等不到数据）。
    pub eof_read: AtomicBool,
}

impl MvPlaybackState {
    pub fn new() -> Self {
        Self {
            shutdown: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            volume: AtomicU32::new(100),
            epoch: AtomicU64::new(0),
            first_pts_us: AtomicU64::new(0),
            first_pts_set: AtomicBool::new(false),
            seek_target_us: AtomicU64::new(NO_SEEK_US),
            buffered_us: AtomicU64::new(0),
            duration_us: AtomicU64::new(0),
            buffering: AtomicBool::new(false),
            eof_read: AtomicBool::new(false),
        }
    }
    pub fn shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
    pub fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
    pub fn reset(&self) {
        self.shutdown.store(false, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
        self.first_pts_us.store(0, Ordering::Release);
        self.first_pts_set.store(false, Ordering::Release);
        self.seek_target_us.store(NO_SEEK_US, Ordering::Release);
        self.buffered_us.store(0, Ordering::Release);
        self.duration_us.store(0, Ordering::Release);
        self.buffering.store(false, Ordering::Release);
        self.eof_read.store(false, Ordering::Release);
    }
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
    pub fn toggle_pause(&self) -> bool {
        let n = !self.paused();
        self.paused.store(n, Ordering::Relaxed);
        n
    }
    pub fn set_volume(&self, v: u32) {
        self.volume.store(v.clamp(0, 100), Ordering::Relaxed);
    }
    pub fn volume(&self) -> u32 {
        self.volume.load(Ordering::Relaxed)
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub fn bump_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::AcqRel) + 1
    }
    pub fn first_pts_us(&self) -> u64 {
        self.first_pts_us.load(Ordering::Acquire)
    }
    /// 视频线程解出首帧时写入（只写一次；用标志而非 0 哨兵，兼容首帧 PTS=0 的流）。
    pub fn set_first_pts_us(&self, v: u64) {
        if !self.first_pts_set.swap(true, Ordering::AcqRel) {
            self.first_pts_us.store(v, Ordering::Release);
        }
    }
    /// 首帧 PTS 基准是否已锁定。
    pub fn first_pts_ready(&self) -> bool {
        self.first_pts_set.load(Ordering::Acquire)
    }
    /// demux seek 时写入目标点（相对会话时间轴）。
    pub fn set_seek_target_us(&self, v: u64) {
        self.seek_target_us.store(v, Ordering::Release);
    }
    /// 当前 seek 目标（无 seek 时为 None）。
    pub fn seek_target_us(&self) -> Option<u64> {
        let v = self.seek_target_us.load(Ordering::Acquire);
        if v == NO_SEEK_US { None } else { Some(v) }
    }
    /// seek 到位（解码出第一帧 ≥ 目标点）后清零，避免残留目标导致
    /// 后续帧/采样被误丢，或下一次 seek 比较基准错误（"seek 只生效一次"根因）。
    pub fn clear_seek_target(&self) {
        self.seek_target_us.store(NO_SEEK_US, Ordering::Release);
    }
    /// demux 每读入一个包就刷新预加载水位（取历史最大值，只进不退）。
    pub fn note_buffered_us(&self, v: u64) {
        self.buffered_us.fetch_max(v, Ordering::Relaxed);
    }
    /// seek 时重置水位（旧位置的预读数据全部作废）。
    pub fn set_buffered_us(&self, v: u64) {
        self.buffered_us.store(v, Ordering::Relaxed);
    }
    pub fn buffered_us(&self) -> u64 {
        self.buffered_us.load(Ordering::Relaxed)
    }
    pub fn duration_us(&self) -> u64 {
        self.duration_us.load(Ordering::Relaxed)
    }
    pub fn set_duration_us(&self, v: u64) {
        self.duration_us.store(v, Ordering::Relaxed);
    }
    pub fn buffering(&self) -> bool {
        self.buffering.load(Ordering::Relaxed)
    }
    pub fn set_buffering(&self, v: bool) {
        self.buffering.store(v, Ordering::Relaxed);
    }
    pub fn eof_read(&self) -> bool {
        self.eof_read.load(Ordering::Relaxed)
    }
    pub fn set_eof_read(&self, v: bool) {
        self.eof_read.store(v, Ordering::Relaxed);
    }
}
