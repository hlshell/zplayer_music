//! MV 音频输出：cpal 推送式 sink + 采样环 + 播放时钟（供视频 pacing 对齐）。
//! 设计内嵌自 rs-wgpu-video-player/player-core 的音频层（精简版）：
//! 解码线程把重采样后的 f32 PCM 推入采样环（环满则阻塞，天然 backpressure），
//! cpal 回调按设备节拍消费。时钟 = 环尾 PTS - 环内积压时长，即"正在响起"的位置。
//! 暂停时回调输出静音且不消费环 → 时钟冻结，视频 pacing 随之停住，恢复后天然同步。
//!
//! 注意：cpal::Stream 非 Send（cpal 跨平台保守标记），因此 MvAudioSink 必须在
//! 音频解码线程内创建；MvAudioClock（Arc 共享核心）可先行创建并跨线程传递。

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::core::mv_state::MvPlaybackState;

/// 时钟基准哨兵（尚未写入任何样本）。
const BASE_UNSET: u64 = u64::MAX;

struct MvAudioCore {
    /// f32 交错采样环。
    ring: Mutex<VecDeque<f32>>,
    /// 时钟基准：会话首个音频样本的容器绝对 PTS（us）。
    /// 固定后不随 seek 变化——与视频 ts_us（abs - 会话首帧）保持同一起点。
    base_abs_us: AtomicU64,
    /// 首个"将被渲染"样本的相对时间（us，相对 base_abs_us）；seek flush 后由下次首块写入重设。
    base_rel_us: AtomicU64,
    /// 已交付给设备的样本总数（回调实际从环中取出的数量；underrun 补零不计）。
    /// 时钟 = base_rel + rendered/sr_total —— 纯计数驱动，天然单调连续，
    /// 不依赖 push 端 PTS 换算，避免部分写入导致的时钟跳变。
    rendered_samples: AtomicU64,
    /// 有有效数据（seek flush 后清除，首次写入后设置）。
    valid: AtomicBool,
    /// 输出采样率/声道数：sink 在线程内创建后回填；0 = 未就绪。
    sample_rate: AtomicU32,
    channels: AtomicU32,
}

/// 音频时钟句柄（视频 pacing 用它对齐）。cheap clone，可跨线程。
#[derive(Clone)]
pub struct MvAudioClock(Arc<MvAudioCore>);

impl MvAudioClock {
    /// 预创建时钟（设备参数未就绪，now_rel_us 返回 None 直到 sink 写入）。
    pub fn new() -> Self {
        Self(Arc::new(MvAudioCore {
            ring: Mutex::new(VecDeque::new()),
            base_abs_us: AtomicU64::new(BASE_UNSET),
            base_rel_us: AtomicU64::new(0),
            rendered_samples: AtomicU64::new(0),
            valid: AtomicBool::new(false),
            sample_rate: AtomicU32::new(0),
            channels: AtomicU32::new(0),
        }))
    }

    /// 音频流结束（EOF/线程退出）时使时钟失效：视频 pacing 回退 wall-clock，
    /// 避免"音频流比视频流短"时时钟冻结导致视频尾部帧永远等不到时钟推进（死锁）。
    pub fn invalidate(&self) {
        self.0.valid.store(false, Ordering::Release);
    }

    /// 当前正在播放位置的相对时间（us）。无有效数据时返回 None（调用方回退 wall-clock）。
    /// 时钟 = 首样本相对 PTS + 已渲染样本时长：回调每从环取出一个样本就累加，
    /// 与 push 端 PTS 换算、环积压、swr 延迟全部解耦，天然单调连续。
    /// 误差仅为设备输出队列延迟（约一个 period，~10ms，恒定不漂移）。
    pub fn now_rel_us(&self) -> Option<u64> {
        let c = &self.0;
        if !c.valid.load(Ordering::Acquire) {
            return None;
        }
        let sr = c.sample_rate.load(Ordering::Acquire) as u64;
        let ch = c.channels.load(Ordering::Acquire) as u64;
        let sr_total = sr * ch;
        if sr_total == 0 {
            return None;
        }
        let rendered_us = c.rendered_samples.load(Ordering::Acquire) * 1_000_000 / sr_total;
        Some(c.base_rel_us.load(Ordering::Acquire).saturating_add(rendered_us))
    }
}

/// cpal 输出流 + 采样环。**非 Send**：必须在音频解码线程内创建并持有；drop 即停止输出。
pub struct MvAudioSink {
    core: Arc<MvAudioCore>,
    cap_samples: usize,
    sample_rate: u32,
    channels: u16,
    _stream: cpal::Stream,
}

impl MvAudioSink {
    /// 打开默认输出设备（采样率/声道跟随设备默认配置，保证兼容性）。
    /// 在音频解码线程内调用。失败（无设备等）返回 None，调用方回退无音频模式。
    pub fn new(state: Arc<MvPlaybackState>, clock: &MvAudioClock) -> Option<Self> {
        let host = cpal::default_host();
        let dev = host.default_output_device()?;
        let supported = dev.default_output_config().ok()?;
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();
        let cfg = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            // 固定 512 帧（@48k ≈ 10.7ms）：Default 在 Windows WASAPI 上可能选到
            // 100ms+ 的大 buffer，导致音频延迟大、音画同步滞后；固定小 buffer 延迟低。
            buffer_size: cpal::BufferSize::Fixed(512),
        };
        let core = clock.0.clone();
        core.sample_rate.store(sample_rate, Ordering::Release);
        core.channels.store(channels as u32, Ordering::Release);
        let cb_core = core.clone();
        let cb_state = state.clone();
        let stream = dev
            .build_output_stream(
                &cfg,
                move |data: &mut [f32], _| {
                    // 暂停 / 缓冲中：输出静音且不消费环（时钟冻结，恢复后音视频仍同步）
                    if cb_state.paused() || cb_state.buffering() {
                        data.fill(0.0);
                        return;
                    }
                    let vol = cb_state.volume() as f32 / 100.0;
                    // 批量 drain：锁内只做一次连续内存搬移（微秒级），立刻放锁再做音量乘法。
                    // 旧实现锁内逐样本 pop 整段回调 → 与解码线程 push 的锁竞争阻塞实时线程，产生爆音。
                    let mut consumed = 0usize;
                    if let Ok(mut ring) = cb_core.ring.lock() {
                        let n = ring.len().min(data.len());
                        for (i, s) in ring.drain(..n).enumerate() {
                            data[i] = s;
                        }
                        consumed = n;
                    }
                    // 已交付样本计数：时钟据此推进（只计真实取到的，underrun 补零不计）
                    if consumed > 0 {
                        cb_core.rendered_samples.fetch_add(consumed as u64, Ordering::Release);
                    }
                    // 音量乘法 + underrun 部分填零（锁外执行，不阻塞解码线程）
                    for s in data[..consumed].iter_mut() {
                        *s *= vol;
                    }
                    data[consumed..].fill(0.0);
                },
                |e| log::warn!(target: "mv_audio", "output error: {e}"),
                None,
            )
            .ok()?;
        stream.play().ok()?;
        log::info!(target: "mv_audio", "output opened: {sample_rate}Hz {channels}ch");
        Some(Self {
            core,
            // 环容量 2s：足以吸收网络 jitter；过大会拉长 push 阻塞唤醒周期，
            // 且远超缓冲门控 4s 恢复阈值没有意义（门控卡住时环照样停）
            cap_samples: (sample_rate as usize) * (channels as usize) * 2,
            sample_rate,
            channels,
            _stream: stream,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// 解码线程推 PCM。first_abs_us / last_abs_us = 本块首/尾样本的容器绝对 PTS（us）。
    /// 环满则阻塞轮询（backpressure）；shutdown 置位时放弃并返回 false。
    /// 时钟只在首块写入时标定基准（first_abs - 会话首帧 PTS），此后纯按"已渲染样本数"推进，
    /// 不再随 push 更新——旧实现按块尾 PTS 更新 tail，部分写入时每次跳变 ~19ms，音画周期性错位。
    pub fn push(&self, samples: &[f32], first_abs_us: u64, last_abs_us: u64,
                state: &MvPlaybackState, epoch: u64) -> bool {
        let _ = last_abs_us; // 时钟不再用块尾 PTS；保留参数以兼容调用方
        let c = &self.core;
        // 时钟零点优先用会话时间轴基准（与视频 ts 同一起点）；未就绪才回退首样本 PTS
        if c.base_abs_us.load(Ordering::Acquire) == BASE_UNSET {
            let base = if state.first_pts_ready() { state.first_pts_us() } else { first_abs_us };
            c.base_abs_us.store(base, Ordering::Release);
        }
        // 首块（seek flush 后的第一次写入）：标定"首个待渲染样本"的相对 PTS 与渲染计数零点
        if !c.valid.load(Ordering::Acquire) {
            let base = c.base_abs_us.load(Ordering::Acquire);
            c.base_rel_us.store(first_abs_us.saturating_sub(base), Ordering::Release);
            c.rendered_samples.store(0, Ordering::Release);
        }
        let mut i = 0;
        while i < samples.len() {
            if state.shutdown() {
                return false;
            }
            // seek 发生：放弃本块剩余旧数据（返回 true 表示"正常结束本块"，
            // 解码线程继续往下走——下一包会走 epoch 变化分支 flush 时钟/环）。
            // 不这样做的话，环满时一次 push 可阻塞到 2s 才醒，seek 响应会明显迟滞。
            if state.epoch() != epoch {
                return true;
            }
            {
                let Ok(mut ring) = c.ring.lock() else { return false };
                let n = self.cap_samples.saturating_sub(ring.len()).min(samples.len() - i);
                if n > 0 {
                    ring.extend(samples[i..i + n].iter().copied());
                    i += n;
                    c.valid.store(true, Ordering::Release);
                }
            }
            if i < samples.len() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        true
    }

    /// seek 时清空采样环、渲染计数归零并使时钟失效（由下一次首块写入重新标定基准）。
    pub fn flush(&self) {
        let c = &self.core;
        if let Ok(mut ring) = c.ring.lock() {
            ring.clear();
        }
        c.valid.store(false, Ordering::Release);
        c.rendered_samples.store(0, Ordering::Release);
        c.base_rel_us.store(0, Ordering::Release);
    }
}
