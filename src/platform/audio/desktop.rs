//! 桌面音频后端：基于 rodio（cpal 输出）的轻量播放器。
//! 每次播放在线程内新建输出流（OutputStream 非 Send，不能跨线程持有），
//! 线程结束自动释放设备；这样天然支持运行时切换输出设备——全局只记录
//! 目标设备名（Send），下次播放按当前设备打开新流。
//! 每个音频用独立 Sink 在独立线程解码播放；全局只存 Arc<Sink>（Send+Sync）。
//! （desktop feature 编译；web/mobile 分别用 WebAudio / 系统媒体实现）

use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::Source;

struct Shared {
    sink: rodio::Sink,
    total_sec: f32,
    /// 续播起点（seek 跳过的秒数）。rodio 的 get_pos() 只统计实际
    /// 流过的采样、不包含 seek 量，真实音频位置 = base_sec + get_pos()。
    base_sec: f32,
}

static CURRENT: Mutex<Option<Arc<Shared>>> = Mutex::new(None);
/// 切歌/seek 过渡标志：点击时置位（旧流已停/即将被替换、新流未就绪），
/// play_at 注册新流后清除。进度轮询据此在过渡期保持 UI 现状（不闪 0、
/// 不闪播放图标）；获取音频失败时由 resolver 调 end_switch 解除。
static SWITCHING: AtomicBool = AtomicBool::new(false);
/// 当前流是否"正常播放到结尾"（sink 播空退出）。播放中途失败/解码中断不会置位。
/// 进度轮询据此区分"真播完 → 自动切下一首"与"中途失败 → 换源重播当前歌"，
/// 避免把本可正常播放、仅短暂中断的歌曲误判为无法播放而强制跳下一首。
static FINISHED: AtomicBool = AtomicBool::new(false);
/// 全局音量 0..1（跨播放持久，新建 sink 时应用）
static VOLUME: Mutex<f32> = Mutex::new(0.8);
/// 目标输出设备名（"默认设备" 或具体设备名；Send，可跨线程）
static DEVICE_NAME: Mutex<String> = Mutex::new(String::new());

/// 打开指定设备（None = 默认设备）构建输出流。返回 (流, 句柄)，由调用线程持有，
/// 线程结束即 drop 释放设备。失败回退默认设备。
fn open_stream(name: Option<&str>) -> (rodio::OutputStream, rodio::OutputStreamHandle) {
    if let Some(name) = name {
        let host = cpal::default_host();
        if let Ok(mut devs) = host.output_devices() {
            if let Some(dev) = devs.find(|d| d.name().map(|n| n == name).unwrap_or(false)) {
                if let Ok(pair) = rodio::OutputStream::try_from_device(&dev) {
                    return pair;
                }
            }
        }
    }
    rodio::OutputStream::try_default().unwrap_or_else(|_| {
        // 无可用设备时返回一个空流（Sink 会静默失败，不 panic）
        rodio::OutputStream::try_default()
            .expect("no audio output device available")
    })
}

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

/// 运行时切换输出设备（记录目标设备名并停止当前播放；下次播放使用新设备）。
pub fn set_output_device(name: &str) -> bool {
    *DEVICE_NAME.lock().unwrap() = name.to_string();
    // 停止当前播放（设备切换后旧流由播放线程结束自动释放）
    stop();
    true
}

/// 当前目标设备名（用于配置持久化）
pub fn current_device_name() -> String {
    DEVICE_NAME.lock().unwrap().clone()
}

/// 播放一段已下载的音频字节，可选从 offset_sec 处续播（seek 到该位置）。
/// offset_sec = 0 时从头播放。
pub fn play_at(bytes: Vec<u8>, total_sec: f32, offset_sec: f32) {
    std::thread::spawn(move || {
        FINISHED.store(false, Ordering::SeqCst);
        let dev = current_device_name();
        let dev = if dev.is_empty() || dev == "默认设备" { None } else { Some(dev) };
        let (_stream, handle) = open_stream(dev.as_deref());
        let Ok(sink) = rodio::Sink::try_new(&handle) else { return; };
        let Ok(mut decoder) = rodio::Decoder::new(Cursor::new(bytes)) else { return; };
        let offset = Duration::from_secs_f32(offset_sec.max(0.0));
        if offset > Duration::ZERO {
            // 随机 seek 即时跳转；格式不支持时回退逐帧解码丢弃（慢但通用）
            if decoder.try_seek(offset).is_err() {
                sink.append(decoder.skip_duration(offset));
            } else {
                sink.append(decoder);
            }
        } else {
            sink.append(decoder);
        }
        // 应用当前音量
        sink.set_volume(*VOLUME.lock().unwrap());
        let shared = Arc::new(Shared { sink, total_sec, base_sec: offset_sec.max(0.0) });
        {
            let mut g = CURRENT.lock().unwrap();
            if let Some(prev) = g.take() {
                prev.sink.stop();
            }
            *g = Some(shared.clone());
            SWITCHING.store(false, Ordering::SeqCst);
        }
        // 阻塞直到播完或被 stop，随后清理自身（若仍是当前流）
        while !shared.sink.empty() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut g = CURRENT.lock().unwrap();
        if g.as_ref().map(|s| Arc::ptr_eq(s, &shared)).unwrap_or(false) {
            // sink 播空退出：正常播放到结尾 → 置"播放完成"标志（非中途失败）
            FINISHED.store(true, Ordering::SeqCst);
            *g = None;
        }
    });
}

/// 停止当前播放并释放输出设备。
pub fn stop() {
    FINISHED.store(false, Ordering::SeqCst);
    if let Ok(mut g) = CURRENT.lock() {
        if let Some(s) = g.take() {
            s.sink.stop();
        }
    }
}

/// 播放/暂停切换。
pub fn toggle() {
    if let Ok(g) = CURRENT.lock() {
        if let Some(s) = g.as_ref() {
            if s.sink.is_paused() {
                s.sink.play();
            } else {
                s.sink.pause();
            }
        }
    }
}

/// 返回 (progress 0..1, is_playing)。无音频或已播完时 is_playing=false。
/// progress 基于真实音频位置（含续播起点 base_sec）。
pub fn status() -> (f32, bool) {
    if let Ok(g) = CURRENT.lock() {
        if let Some(s) = g.as_ref() {
            if s.total_sec > 0.0 {
                let pos = s.base_sec + s.sink.get_pos().as_secs_f32();
                let playing = !s.sink.is_paused() && !s.sink.empty();
                return ((pos / s.total_sec).clamp(0.0, 1.0), playing);
            }
        }
    }
    (0.0, false)
}

/// 当前是否有活动播放流（切歌/seek 过渡期为 false）。
pub fn active() -> bool {
    CURRENT.lock().map(|g| g.is_some()).unwrap_or(false)
}

/// 标记进入切换过渡态（点击切歌/seek/续播时调用，点击即刻反馈不闪 UI）。
pub fn begin_switch() {
    SWITCHING.store(true, Ordering::SeqCst);
    FINISHED.store(false, Ordering::SeqCst);
}

/// 当前流是否已正常播放到结尾（自动切下一首的依据）。
pub fn finished() -> bool {
    FINISHED.load(Ordering::SeqCst)
}

/// 解除切换过渡态（获取音频失败时调用，让轮询复位显示）。
pub fn end_switch() {
    SWITCHING.store(false, Ordering::SeqCst);
}

/// 是否处于切换过渡态。
pub fn switching() -> bool {
    SWITCHING.load(Ordering::SeqCst)
}

/// 设置音量 0..1（持久保存并应用到当前 sink）。
pub fn set_volume(v: f32) {
    let v = v.clamp(0.0, 1.0);
    *VOLUME.lock().unwrap() = v;
    if let Ok(g) = CURRENT.lock() {
        if let Some(s) = g.as_ref() {
            s.sink.set_volume(v);
        }
    }
}

/// 读取当前音量 0..1。
pub fn get_volume() -> f32 {
    *VOLUME.lock().unwrap()
}
