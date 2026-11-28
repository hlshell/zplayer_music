//! 桌面音频后端：常驻 cpal 输出流 + 独立解码线程 + 无锁环。
//!
//! 设计目标：系统被外部进程占满 CPU（任务管理器里本进程只占 1%~2%）时依然不断音。
//!
//! - 解码/重采样/格式转换全部在解码线程完成（`rodio::Decoder` +
//!   `UniformSourceIterator`，与旧实现同一套转换，音质不变）；
//! - 实时回调只做「无锁环搬样本 + 乘音量 + 计数」，不加锁、不分配、不解码。
//!   旧实现基于 `rodio::Sink`，解码与多处 Mutex 都发生在 cpal 回调里，
//!   一旦被满载系统抢占就会欠载（断音/爆音），且与 UI 线程形成优先级反转；
//! - 输出流常驻（仅切设备时重建），切歌不再重新初始化 WASAPI；
//! - 环容量约 2s、起播预填充 200ms，吸收调度抖动；
//! - 位置/暂停/播放结束全走原子量，UI 250ms 轮询不再与实时线程抢锁。

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rodio::source::UniformSourceIterator;
use rodio::{Decoder, Source};

use crate::core::audio_ring::SpscRing;

/// 环容量（样本数，交错）：48k/立体声时约 2s。容量固定，实际秒数随设备采样率浮动。
const RING_SAMPLES: usize = 192_000;
/// 起播前预填充时长：先攒够再放行，避免刚起播就撞上一次调度抖动。
const PREFILL_MS: u64 = 200;
/// 解码线程单块时长：一次解出约 40ms，减少循环与加锁次数。
const CHUNK_MS: u64 = 40;
/// WASAPI 共享模式固定缓冲（约 21ms@48k）；设备不接受时回退 `Default`。
const DEVICE_BUFFER_FRAMES: u32 = 1024;

/// 播放引擎共享状态：实时回调、解码线程、UI 轮询三方的唯一交汇点。
/// 全部字段均为原子量或无锁环；`sync` 只用于「生产者写入」与「回调清环」互斥，
/// 回调侧只用 `try_lock`，绝不会阻塞实时线程。
struct Engine {
    ring: SpscRing,
    /// 环内容代数：`play_at`/`stop` 递增；回调发现不一致就丢弃旧数据。
    generation: AtomicU64,
    /// 回调已完成清环的代数（生产者据此确认可以写入本代际数据）。
    synced_gen: AtomicU64,
    sync: Mutex<()>,
    /// 本代际已交付给设备的样本数（进度 = base_sec + rendered / (sr*ch)）。
    rendered: AtomicU64,
    /// 已攒够预填充量、可以开始出声。
    primed: AtomicBool,
    paused: AtomicBool,
    active: AtomicBool,
    /// 已自然播完（EOF 且环已空）。
    drained: AtomicBool,
    /// 正常播完标志（供自动连播判定，与「中途失败」区分）。
    finished: AtomicBool,
    /// 切歌/seek 过渡态（保持原有 UI 语义）。
    switching: AtomicBool,
    /// 解码线程写完最后一块时写入本代际号；回调据此判定结束。
    eof_gen: AtomicU64,
    total_sec: AtomicU32,
    base_sec: AtomicU32,
    sample_rate: AtomicU32,
    channels: AtomicU32,
    volume: AtomicU32,
    /// 回声用欠载计数（回调补零的样本数），用于验证抗抢占效果。
    underruns: AtomicU64,
}

fn engine() -> &'static Arc<Engine> {
    static E: OnceLock<Arc<Engine>> = OnceLock::new();
    E.get_or_init(|| {
        Arc::new(Engine {
            ring: SpscRing::with_capacity(RING_SAMPLES),
            generation: AtomicU64::new(0),
            synced_gen: AtomicU64::new(0),
            sync: Mutex::new(()),
            rendered: AtomicU64::new(0),
            primed: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            active: AtomicBool::new(false),
            drained: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            switching: AtomicBool::new(false),
            eof_gen: AtomicU64::new(0),
            total_sec: AtomicU32::new(0.0f32.to_bits()),
            base_sec: AtomicU32::new(0.0f32.to_bits()),
            sample_rate: AtomicU32::new(0),
            channels: AtomicU32::new(0),
            volume: AtomicU32::new(0.8f32.to_bits()),
            underruns: AtomicU64::new(0),
        })
    })
}

#[derive(Clone, Copy)]
struct DevInfo {
    sample_rate: u32,
    channels: u16,
}

// #region 输出线程：常驻 cpal 流（cpal::Stream 非 Send，只能由创建它的线程持有/释放）

enum OutCmd {
    /// 确保输出流指向目标设备；已指向则直接返回当前参数。
    Ensure {
        device: Option<String>,
        reply: Sender<Result<DevInfo, String>>,
    },
}

fn output_tx() -> &'static Sender<OutCmd> {
    static TX: OnceLock<Sender<OutCmd>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<OutCmd>();
        std::thread::spawn(move || output_loop(rx));
        tx
    })
}

fn output_loop(rx: mpsc::Receiver<OutCmd>) {
    let mut current: Option<(cpal::Stream, String, DevInfo)> = None;
    while let Ok(OutCmd::Ensure { device, reply }) = rx.recv() {
        let key = device_key(device.as_deref());
        if let Some((_, cur_key, info)) = current.as_ref() {
            if *cur_key == key {
                let _ = reply.send(Ok(*info));
                continue;
            }
        }
        // 旧流必须在当前线程 drop（cpal::Stream 非 Send）
        current = None;
        match build_stream(device.as_deref()) {
            Ok((stream, info)) => {
                let _ = reply.send(Ok(info));
                current = Some((stream, key, info));
            }
            Err(err) => {
                log::warn!(target: "audio", "打开输出流失败：{err}");
                let _ = reply.send(Err(err));
            }
        }
    }
}

/// 设备键：默认设备解析成实际设备名，这样系统默认设备变化时能触发重建。
fn device_key(device: Option<&str>) -> String {
    match device {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => cpal::default_host()
            .default_output_device()
            .and_then(|d| d.name().ok())
            .unwrap_or_default(),
    }
}

fn build_stream(name: Option<&str>) -> Result<(cpal::Stream, DevInfo), String> {
    let host = cpal::default_host();
    let device = match name {
        Some(n) => match host
            .output_devices()
            .ok()
            .and_then(|mut it| it.find(|d| d.name().map(|x| x == n).unwrap_or(false)))
        {
            Some(d) => d,
            None => host
                .default_output_device()
                .ok_or_else(|| format!("找不到输出设备：{n}"))?,
        },
        None => host
            .default_output_device()
            .ok_or_else(|| "没有可用的音频输出设备".to_string())?,
    };
    let supported = device
        .default_output_config()
        .map_err(|e| format!("读取设备默认配置失败：{e}"))?;
    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels();
    let eng = engine();
    // 先写好输出参数，再启动流，避免回调先跑而进度换算拿到 0
    eng.sample_rate.store(sample_rate, Ordering::Release);
    eng.channels.store(channels as u32, Ordering::Release);

    let mut built = None;
    for buffer_size in [
        cpal::BufferSize::Fixed(DEVICE_BUFFER_FRAMES),
        cpal::BufferSize::Default,
    ] {
        let cfg = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size,
        };
        let cb_eng = eng.clone();
        let result = device.build_output_stream(
            &cfg,
            move |data: &mut [f32], _| audio_callback(&cb_eng, data),
            |e| log::warn!(target: "audio", "输出流错误：{e}"),
            None,
        );
        match result {
            Ok(stream) => {
                built = Some(stream);
                break;
            }
            Err(e) => log::warn!(target: "audio", "输出流配置不被接受：{e}"),
        }
    }
    let stream = built.ok_or_else(|| "打开音频输出流失败".to_string())?;
    stream
        .play()
        .map_err(|e| format!("启动音频输出失败：{e}"))?;
    Ok((stream, DevInfo { sample_rate, channels }))
}

fn ensure_stream(device: Option<String>) -> Result<DevInfo, String> {
    let (tx, rx) = mpsc::channel();
    output_tx()
        .send(OutCmd::Ensure {
            device,
            reply: tx,
        })
        .map_err(|_| "音频输出线程不可用".to_string())?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|_| "音频设备初始化超时".to_string())?
}

// #endregion

/// 实时音频回调：零锁（除清环时的一次 try_lock）、零分配、零解码。
fn audio_callback(e: &Engine, data: &mut [f32]) {
    register_mmcss_once();

    // 代际变化：丢弃旧歌残留数据（生产者此刻要么没开写、要么短暂持锁，try_lock 失败下轮再清）
    let generation = e.generation.load(Ordering::Acquire);
    if generation != e.synced_gen.load(Ordering::Relaxed) {
        match e.sync.try_lock() {
            Ok(_guard) => {
                e.ring.discard();
                e.rendered.store(0, Ordering::Release);
                e.primed.store(false, Ordering::Release);
                e.synced_gen.store(generation, Ordering::Release);
            }
            Err(_) => {
                data.fill(0.0);
                return;
            }
        }
    }

    if e.paused.load(Ordering::Relaxed)
        || !e.active.load(Ordering::Relaxed)
        || !e.primed.load(Ordering::Relaxed)
    {
        data.fill(0.0);
        return;
    }

    let n = e.ring.pop(data);
    if n > 0 {
        let vol = f32::from_bits(e.volume.load(Ordering::Relaxed));
        if vol != 1.0 {
            for s in &mut data[..n] {
                *s *= vol;
            }
        }
        e.rendered.fetch_add(n as u64, Ordering::Release);
    }
    if n < data.len() {
        data[n..].fill(0.0);
        // 非结尾处的补零都算欠载（调度抖动/解码跟不上），仅计数便于回归验证
        if e.eof_gen.load(Ordering::Acquire) != generation {
            e.underruns
                .fetch_add((data.len() - n) as u64, Ordering::Relaxed);
        }
    }

    // 结束判定：解码已写完 + 环已空
    if n == 0 && e.eof_gen.load(Ordering::Acquire) == generation {
        e.active.store(false, Ordering::Release);
        e.drained.store(true, Ordering::Release);
        e.finished.store(true, Ordering::Release);
    }
}

/// 解码线程：把整曲解码成设备格式 PCM，按块写入无锁环（环满退避轮询）。
fn decode_thread(e: Arc<Engine>, generation: u64, bytes: Vec<u8>, offset_sec: f32) {
    raise_thread_priority();

    let dev = current_device_name();
    let dev = if dev.is_empty() || dev == "默认设备" {
        None
    } else {
        Some(dev)
    };
    let info = match ensure_stream(dev) {
        Ok(info) => info,
        Err(err) => {
            log::warn!(target: "audio", "音频设备不可用：{err}");
            fail_track(&e, generation);
            return;
        }
    };

    // 等回调把环切到本代际，否则会被回调当成旧数据清掉
    let deadline = Instant::now() + Duration::from_millis(800);
    while e.synced_gen.load(Ordering::Acquire) != generation {
        if e.generation.load(Ordering::Acquire) != generation {
            return;
        }
        if Instant::now() > deadline {
            log::warn!(target: "audio", "音频输出未就绪，放弃本曲");
            fail_track(&e, generation);
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let mut decoder = match Decoder::new(Cursor::new(bytes)) {
        Ok(d) => d,
        Err(err) => {
            log::warn!(target: "audio", "音频解码器创建失败：{err}");
            fail_track(&e, generation);
            return;
        }
    };
    let channels = info.channels;
    let rate = info.sample_rate;
    if channels == 0 || rate == 0 {
        fail_track(&e, generation);
        return;
    }

    // 随机 seek 优先（即时）；格式不支持时回退逐帧解码丢弃
    let offset = Duration::from_secs_f32(offset_sec.max(0.0));
    let seek_ok = offset.is_zero() || decoder.try_seek(offset).is_ok();
    let mut src: Box<dyn Iterator<Item = f32> + Send> = if seek_ok {
        Box::new(UniformSourceIterator::<_, f32>::new(decoder, channels, rate))
    } else {
        Box::new(UniformSourceIterator::<_, f32>::new(
            decoder.skip_duration(offset),
            channels,
            rate,
        ))
    };

    let frame_samples = channels as usize;
    let chunk = (rate as usize * frame_samples / 1000 * CHUNK_MS as usize).max(256);
    let prefill = (rate as usize * frame_samples * PREFILL_MS as usize / 1000).max(1);
    let mut buf = vec![0f32; chunk];
    let mut produced = 0usize;

    loop {
        if e.generation.load(Ordering::Acquire) != generation {
            return;
        }
        let mut count = 0usize;
        for slot in buf.iter_mut() {
            match src.next() {
                Some(v) => {
                    *slot = v;
                    count += 1;
                }
                None => break,
            }
        }
        if count == 0 {
            // 播完：环里还有积压，由回调播空后收尾
            if e.generation.load(Ordering::Acquire) == generation {
                e.eof_gen.store(generation, Ordering::Release);
                // 曲目短于预填充量时直接放行，否则永远等不到 primed
                e.primed.store(true, Ordering::Release);
                timer_resolution_boost(false);
            }
            return;
        }

        let mut off = 0usize;
        while off < count {
            if e.generation.load(Ordering::Acquire) != generation {
                return;
            }
            let written = {
                let _guard = e.sync.lock().unwrap_or_else(|p| p.into_inner());
                if e.synced_gen.load(Ordering::Acquire) == generation {
                    e.ring.push(&buf[off..count])
                } else {
                    0
                }
            };
            off += written;
            if off < count {
                // 环满：退避等待回调消费（backpressure），期间保持响应切歌
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        produced += count;
        if !e.primed.load(Ordering::Relaxed) && produced >= prefill {
            e.primed.store(true, Ordering::Release);
        }
    }
}

/// 本曲失败（无设备/解码失败/输出未就绪）：复位状态并解除 UI 过渡态，
/// 让 250ms 轮询按「中途失败」分支走换源重播，而不是永久冻结。
fn fail_track(e: &Engine, generation: u64) {
    if e.generation.load(Ordering::Acquire) != generation {
        return;
    }
    e.active.store(false, Ordering::Release);
    e.drained.store(false, Ordering::Release);
    e.finished.store(false, Ordering::Release);
    e.switching.store(false, Ordering::Release);
    e.primed.store(false, Ordering::Release);
    timer_resolution_boost(false);
}

// #region debug-point:transport

static DEBUG_STREAM_SEQ: AtomicU64 = AtomicU64::new(1);

fn debug_sender() -> &'static std::sync::mpsc::Sender<serde_json::Value> {
    static TX: std::sync::OnceLock<std::sync::mpsc::Sender<serde_json::Value>> = std::sync::OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            let env = std::fs::read_to_string(r"C:\Users\93453\AppData\Local\Temp\trae-debug\slint-demo\playback-sudden-stop\playback-sudden-stop.env").unwrap_or_default();
            let url = env.lines().find_map(|line| line.strip_prefix("DEBUG_SERVER_URL=")).unwrap_or("http://127.0.0.1:7777/event").to_string();
            let client = reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_millis(250)).build().ok();
            while let Ok(event) = rx.recv() {
                if let Some(client) = &client {
                    let _ = client.post(&url).json(&event).send();
                }
            }
        });
        tx
    })
}

pub(crate) fn debug_event(hypothesis_id: &'static str, location: &'static str, msg: &'static str, data: serde_json::Value) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64;
    let run_id = std::env::var("TRAE_DEBUG_RUN_ID").unwrap_or_else(|_| "pre-fix".to_string());
    let event = serde_json::json!({"sessionId":"playback-sudden-stop","runId":run_id,"hypothesisId":hypothesis_id,"location":location,"msg":format!("[DEBUG] {msg}"),"data":data,"ts":ts});
    let _ = debug_sender().send(event);
}

// #endregion

/// 枚举系统音频输出设备（首个为"默认设备"）。
pub fn list_devices() -> Vec<String> {
    let mut v = vec!["默认设备".to_string()];
    let host = cpal::default_host();
    if let Ok(devs) = host.output_devices() {
        for d in devs {
            if let Ok(n) = d.name() {
                v.push(n);
            }
        }
    }
    v
}

/// 运行时切换输出设备（停止当前播放，下次播放按新设备重建输出流）。
#[track_caller]
pub fn set_output_device(name: &str) -> bool {
    debug_event(
        "H2",
        "desktop.rs:set_output_device",
        "output device switch requested",
        serde_json::json!({"caller":std::panic::Location::caller().to_string(),"device":name,"before":debug_snapshot()}),
    );
    *DEVICE_NAME.lock().unwrap() = name.to_string();
    stop();
    true
}

/// 当前目标设备名（用于配置持久化）
pub fn current_device_name() -> String {
    DEVICE_NAME.lock().unwrap().clone()
}

/// 播放一段已下载的音频字节，可选从 offset_sec 处续播（seek 到该位置）。
/// offset_sec = 0 时从头播放。解码与写环在独立线程完成，本函数立即返回。
#[track_caller]
pub fn play_at(bytes: Vec<u8>, total_sec: f32, offset_sec: f32) {
    let e = engine().clone();
    let generation = e.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let stream_id = DEBUG_STREAM_SEQ.fetch_add(1, Ordering::Relaxed);
    e.total_sec
        .store(total_sec.max(0.0).to_bits(), Ordering::Release);
    e.base_sec
        .store(offset_sec.max(0.0).to_bits(), Ordering::Release);
    e.rendered.store(0, Ordering::Release);
    e.primed.store(false, Ordering::Release);
    e.paused.store(false, Ordering::Release);
    e.drained.store(false, Ordering::Release);
    e.finished.store(false, Ordering::Release);
    e.switching.store(false, Ordering::Release);
    e.active.store(true, Ordering::Release);
    // 播放期间提高定时器精度：满载时 15.6ms 的睡眠粒度会放大调度抖动
    timer_resolution_boost(true);

    debug_event(
        "H3",
        "desktop.rs:play_at",
        "play requested",
        serde_json::json!({"stream_id":stream_id,"generation":generation,"caller":std::panic::Location::caller().to_string(),"bytes":bytes.len(),"total_sec":total_sec,"offset_sec":offset_sec,"before":debug_snapshot()}),
    );

    std::thread::spawn(move || decode_thread(e, generation, bytes, offset_sec.max(0.0)));
}

/// 停止当前播放（保留常驻输出流，仅清空环并递增代际）。
#[track_caller]
pub fn stop() {
    let e = engine();
    let before = debug_snapshot();
    e.generation.fetch_add(1, Ordering::SeqCst);
    e.active.store(false, Ordering::Release);
    e.paused.store(false, Ordering::Release);
    e.finished.store(false, Ordering::Release);
    e.drained.store(false, Ordering::Release);
    e.primed.store(false, Ordering::Release);
    timer_resolution_boost(false);
    debug_event(
        "H2",
        "desktop.rs:stop",
        "explicit stop",
        serde_json::json!({"caller":std::panic::Location::caller().to_string(),"before":before,"after":debug_snapshot()}),
    );
}

/// 播放/暂停切换（纯原子操作，不惊动解码线程）。
#[track_caller]
pub fn toggle() {
    let e = engine();
    let before = debug_snapshot();
    if e.active.load(Ordering::Acquire) {
        let paused = e.paused.load(Ordering::Acquire);
        e.paused.store(!paused, Ordering::Release);
    }
    debug_event(
        "H2",
        "desktop.rs:toggle",
        "explicit toggle",
        serde_json::json!({"caller":std::panic::Location::caller().to_string(),"before":before,"after":debug_snapshot()}),
    );
}

/// 返回 (progress 0..1, is_playing)。无音频或已播完时 is_playing=false。
pub fn status() -> (f32, bool) {
    let e = engine();
    if !e.active.load(Ordering::Acquire) {
        return (0.0, false);
    }
    let total = f32::from_bits(e.total_sec.load(Ordering::Acquire));
    if total <= 0.0 {
        return (0.0, false);
    }
    // 注意：这里不看预填充标志——起播前 200ms 虽然在攒数据，UI 语义上仍是"播放中"，
    // 否则这段时间点暂停会被误判成"重新起播"。
    let playing = !e.paused.load(Ordering::Acquire) && !e.drained.load(Ordering::Acquire);
    ((position_sec(e) / total).clamp(0.0, 1.0), playing)
}

fn position_sec(e: &Engine) -> f32 {
    let denom =
        e.sample_rate.load(Ordering::Acquire) as u64 * e.channels.load(Ordering::Acquire) as u64;
    let base = f32::from_bits(e.base_sec.load(Ordering::Acquire));
    if denom == 0 {
        return base;
    }
    base + e.rendered.load(Ordering::Acquire) as f32 / denom as f32
}

// #region debug-point:snapshot
pub(crate) fn debug_snapshot() -> serde_json::Value {
    let e = engine();
    let active = e.active.load(Ordering::Acquire);
    let paused = e.paused.load(Ordering::Acquire);
    let buffered = e.ring.len();
    serde_json::json!({
        "stream_id": e.generation.load(Ordering::Acquire),
        "active": active,
        "playing": active && !paused && !e.drained.load(Ordering::Acquire),
        "paused": paused,
        "empty": buffered == 0,
        "pos_sec": position_sec(e),
        "total_sec": f32::from_bits(e.total_sec.load(Ordering::Acquire)),
        "base_sec": f32::from_bits(e.base_sec.load(Ordering::Acquire)),
        "switching": e.switching.load(Ordering::Acquire),
        "finished": e.finished.load(Ordering::Acquire),
        "buffered_samples": buffered,
        "underrun_samples": e.underruns.load(Ordering::Relaxed),
        "lock_error": false,
    })
}
// #endregion

/// 当前流是否处于用户暂停态。
pub fn paused() -> bool {
    engine().paused.load(Ordering::Acquire)
}

/// 当前是否有活动播放流（切歌/seek 过渡期为 false）。
pub fn active() -> bool {
    engine().active.load(Ordering::Acquire)
}

/// 标记进入切换过渡态（点击切歌/seek/续播时调用，点击即刻反馈不闪 UI）。
pub fn begin_switch() {
    let e = engine();
    e.switching.store(true, Ordering::Release);
    e.finished.store(false, Ordering::Release);
}

/// 当前流是否已正常播放到结尾（自动切下一首的依据）。
pub fn finished() -> bool {
    engine().finished.load(Ordering::Acquire)
}

/// 解除切换过渡态（获取音频失败时调用，让轮询复位显示）。
pub fn end_switch() {
    engine().switching.store(false, Ordering::Release);
}

/// 是否处于切换过渡态。
pub fn switching() -> bool {
    engine().switching.load(Ordering::Acquire)
}

/// 设置音量 0..1（持久保存，回调按原子量即时生效）。
pub fn set_volume(v: f32) {
    engine()
        .volume
        .store(v.clamp(0.0, 1.0).to_bits(), Ordering::Release);
}

/// 读取当前音量 0..1。
pub fn get_volume() -> f32 {
    f32::from_bits(engine().volume.load(Ordering::Acquire))
}

/// 目标输出设备名（"默认设备" 或具体设备名；Send，可跨线程）
static DEVICE_NAME: Mutex<String> = Mutex::new(String::new());

// #region 系统侧抗干扰

/// 进程级一次性调优：关闭 EcoQoS（后台不被降频/降定时器精度）、进程优先级提到 AboveNormal。
/// 只提高一档而不设 High，避免反过来饿死系统里其他进程。
#[cfg(windows)]
pub fn init_process_tuning() {
    unsafe {
        use winapi::um::processthreadsapi::{GetCurrentProcess, SetPriorityClass};
        use winapi::um::winbase::ABOVE_NORMAL_PRIORITY_CLASS;
        SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS);
        apply_power_throttling_opt_out();
    }
}

#[cfg(not(windows))]
pub fn init_process_tuning() {}

#[cfg(windows)]
unsafe fn apply_power_throttling_opt_out() {
    // winapi 0.3.9 未收录动力节流结构/枚举值，这里直接声明最小所需定义。
    #[repr(C)]
    struct PowerThrottlingState {
        version: u32,
        control_mask: u32,
        state_mask: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        // 用 winapi 的 c_void 别名，与 GetCurrentProcess() 的指针类型保持一致
        fn SetProcessInformation(
            process: *mut winapi::ctypes::c_void,
            class: i32,
            info: *mut winapi::ctypes::c_void,
            size: u32,
        ) -> i32;
    }
    use winapi::um::processthreadsapi::GetCurrentProcess;

    const PROCESS_POWER_THROTTLING_CURRENT_VERSION: u32 = 1;
    const PROCESS_POWER_THROTTLING_EXECUTION_SPEED: u32 = 0x1;
    const PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION: u32 = 0x4;
    const PROCESS_POWER_THROTTLING: i32 = 4;

    let mut state = PowerThrottlingState {
        version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        control_mask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        // StateMask 清零 = 关闭上述两种节流
        state_mask: 0,
    };
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            PROCESS_POWER_THROTTLING,
            &mut state as *mut _ as *mut winapi::ctypes::c_void,
            std::mem::size_of::<PowerThrottlingState>() as u32,
        )
    };
    if ok == 0 {
        log::debug!(target: "audio", "关闭电源节流失败（系统不支持则忽略）");
    }
}

/// 播放期间把系统定时器精度提到 1ms（Win10 2004+ 按进程生效）。
/// 满载时 15.6ms 的默认粒度会让睡眠/等待抖动被放大成 audible gap。
#[cfg(windows)]
pub fn timer_resolution_boost(on: bool) {
    use std::sync::atomic::AtomicBool;
    use winapi::um::timeapi::{timeBeginPeriod, timeEndPeriod};
    static BOOSTED: AtomicBool = AtomicBool::new(false);
    if on {
        if !BOOSTED.swap(true, Ordering::SeqCst) {
            unsafe { timeBeginPeriod(1) };
        }
    } else if BOOSTED.swap(false, Ordering::SeqCst) {
        unsafe { timeEndPeriod(1) };
    }
}

#[cfg(not(windows))]
pub fn timer_resolution_boost(_on: bool) {}

/// 解码线程提到 AboveNormal：满载时优先拿到 CPU 把环填满。
#[cfg(windows)]
fn raise_thread_priority() {
    unsafe {
        use winapi::um::processthreadsapi::{GetCurrentThread, SetThreadPriority};
        use winapi::um::winbase::THREAD_PRIORITY_ABOVE_NORMAL;
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL as i32);
    }
}

#[cfg(not(windows))]
fn raise_thread_priority() {}

/// 音频回调线程注册 MMCSS（Windows 多媒体类调度，音频路径的标准做法）。
/// cpal 已把该线程设为 TIME_CRITICAL，MMCSS 进一步保证被抢占后尽快回到运行态。
#[cfg(windows)]
fn register_mmcss_once() {
    use std::cell::Cell;
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    let _ = DONE.try_with(|done| {
        if !done.get() {
            done.set(true);
            unsafe { mmcss_attach() };
        }
    });
}

#[cfg(not(windows))]
fn register_mmcss_once() {}

#[cfg(windows)]
unsafe fn mmcss_attach() {
    #[link(name = "avrt")]
    unsafe extern "system" {
        fn AvSetMmThreadCharacteristicsW(
            task_name: *const u16,
            task_index: *mut u32,
        ) -> *mut core::ffi::c_void;
    }
    // "Audio\0"
    let name: [u16; 6] = [0x41, 0x75, 0x64, 0x69, 0x6f, 0];
    let mut index: u32 = 0;
    // 句柄随线程存活即可；音频回调线程生命周期等同进程，不做 AvRevert
    unsafe {
        let _ = AvSetMmThreadCharacteristicsW(name.as_ptr(), &mut index);
    }
}

// #endregion
