//! 共享 rodio 播放核心：供移动端复用桌面同款解码/输出实现。
//!
//! 与 `desktop.rs` 的区别：不涉及输出设备枚举与切换，一律使用系统默认设备。
//! 桌面端现有实现暂未改为委托本模块（其工作区存在进行中的调试探针），待合并后统一。
//!
//! 每次播放在独立线程新建输出流（`OutputStream` 非 Send，不能跨线程持有），
//! 线程结束自动释放设备；每个音频用独立 Sink 解码播放，全局只存 Arc<Sink>。

// 门面由 mobile.rs re-export，阶段 1 尚无业务调用者（阶段 3 接入播放后自然使用）
#![allow(dead_code)]

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
/// play_at 注册新流后清除。进度轮询据此在过渡期保持 UI 现状。
static SWITCHING: AtomicBool = AtomicBool::new(false);
/// 当前流是否"正常播放到结尾"（sink 播空退出）。
static FINISHED: AtomicBool = AtomicBool::new(false);
/// 全局音量 0..1（跨播放持久，新建 sink 时应用）
static VOLUME: Mutex<f32> = Mutex::new(0.8);

/// 播放一段已下载的音频字节，可选从 offset_sec 处续播（seek 到该位置）。
/// offset_sec = 0 时从头播放。无可用输出设备时静默放弃本次播放。
pub fn play_at(bytes: Vec<u8>, total_sec: f32, offset_sec: f32) {
    std::thread::spawn(move || {
        FINISHED.store(false, Ordering::SeqCst);
        let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
            return;
        };
        let Ok(sink) = rodio::Sink::try_new(&handle) else {
            return;
        };
        let Ok(mut decoder) = rodio::Decoder::new(Cursor::new(bytes)) else {
            return;
        };
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
        let shared = Arc::new(Shared {
            sink,
            total_sec,
            base_sec: offset_sec.max(0.0),
        });
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
