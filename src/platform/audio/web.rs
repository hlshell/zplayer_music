//! Web 音频后端（阶段 1a）：HTMLAudioElement 播放。
//! 实现 audio 门面契约（play_at/stop/toggle/status/.../set_volume/...）。
//! wasm 单线程，用 thread_local 持有元素与状态；播放字节用 Blob URL。
use std::cell::RefCell;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{window, Blob, HtmlAudioElement, Url};

thread_local! {
    static AUDIO: RefCell<Option<HtmlAudioElement>> = const { RefCell::new(None) };
    // 默认音量 0.8（与 desktop 配置默认一致）。此前默认 0.0 且 web_main 从未 set_volume
    // → start_play 里 el.set_volume(0.0) → HTMLAudio 静音播放（"没声音"根因）。
    static STATE: RefCell<State> = const {
        RefCell::new(State { volume: 0.8, total: 0.0, playing: false, switching: false, finished: false })
    };
}

#[derive(Clone, Copy, Default)]
struct State {
    volume: f32,
    total: f32,
    playing: bool,
    switching: bool,
    finished: bool,
}

fn audio() -> Option<HtmlAudioElement> {
    AUDIO.with(|a| a.borrow().clone())
}

fn ensure_audio() -> HtmlAudioElement {
    AUDIO.with(|a| {
        let mut g = a.borrow_mut();
        if g.is_none() {
            *g = Some(build_audio_element());
        }
        g.clone().unwrap()
    })
}

fn build_audio_element() -> HtmlAudioElement {
    let Some(win) = window() else { panic!("web: no window") };
    let Some(doc) = win.document() else { panic!("web: no document") };
    let el: HtmlAudioElement = doc
        .create_element("audio")
        .ok()
        .and_then(|d| d.dyn_into().ok())
        .expect("create audio element");
    el.set_preload("none");
    // 挂载到 document.body：不挂载的 audio 元素虽可播放但无法从 JS/DOM 检出
    // （浏览器实测与调试都不可见），挂载后 querySelector('audio') 可验证播放状态。
    let _ = win.document().and_then(|d| d.body()).map(|body| body.append_child(el.unchecked_ref()));
    // 播放结束：更新 STATE（finished/playing），播放器轮询 audio::finished() 推进队列。
    // 此前只写局部 Rc 无人读取 → 队列永不自动切歌。
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |_e: web_sys::Event| {
        STATE.with(|s| {
            let mut g = s.borrow_mut();
            g.finished = true;
            g.playing = false;
        });
    });
    el.set_onended(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
    el
}

// ---- 门面契约 ----
pub fn list_devices() -> Vec<String> {
    Vec::new() // web 无设备枚举
}
pub fn set_output_device(_name: &str) -> bool {
    false
}
pub fn current_device_name() -> String {
    "默认设备".into()
}

pub fn play_at(bytes: Vec<u8>, total_sec: f32, offset_sec: f32) {
    let el = ensure_audio();
    // bytes → Uint8Array → Blob → ObjectURL
    let u8a = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    u8a.copy_from(&bytes);
    let parts = js_sys::Array::new();
    parts.push(&u8a);
    let Ok(blob) = Blob::new_with_u8_array_sequence(&parts) else { return };
    if let Ok(url) = Url::create_object_url_with_blob(&blob) {
        start_play(el, &url, total_sec, offset_sec);
    }
}

/// web 播放 gdmusic/源解析返回的音频 URL（HTMLAudio 播外链不需要 CORS）。
/// web 主入口（wasm）用此播放，不走 play_at(bytes) 的 Blob 路径。
pub fn play_url(url: &str, total_sec: f32) {
    let el = ensure_audio();
    start_play(el, url, total_sec, 0.0);
}

fn start_play(el: HtmlAudioElement, url: &str, total_sec: f32, offset_sec: f32) {
    el.set_src(url);
    if offset_sec > 0.0 {
        el.set_current_time(offset_sec as f64);
    }
    el.set_volume(STATE.with(|s| s.borrow().volume.clamp(0.0, 1.0) as f64));
    // play() 在浏览器返回 Promise：自动播放策略/格式不支持/网络错误会 reject 且默认静默
    // （表现 = "没声音"却无任何报错）。spawn_local 驱动 Promise，reject 时上报 console。
    if let Ok(p) = el.play() {
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = wasm_bindgen_futures::JsFuture::from(p).await {
                web_sys::console::log_1(&format!("[web-audio] play() 被浏览器拒绝: {e:?}").into());
            }
        });
    }
    STATE.with(|s| {
        let mut g = s.borrow_mut();
        g.total = total_sec;
        g.playing = true;
        g.finished = false;
    });
}

pub fn stop() {
    if let Some(el) = audio() {
        let _ = el.pause();
        el.set_current_time(0.0);
    }
    STATE.with(|s| {
        let mut g = s.borrow_mut();
        g.playing = false;
        g.finished = true;
    });
}

pub fn toggle() {
    if let Some(el) = audio() {
        if el.paused() {
            let _ = el.play();
            STATE.with(|s| s.borrow_mut().playing = true);
        } else {
            let _ = el.pause();
            STATE.with(|s| s.borrow_mut().playing = false);
        }
    }
}

/// 跳转播放进度（HTMLAudioElement 原生支持；web 端 seek 门面，play.rs 调用）。
pub fn seek(sec: f32) {
    if let Some(el) = audio() {
        el.set_current_time(sec.max(0.0) as f64);
    }
}

pub fn status() -> (f32, bool) {
    let pos = audio().map(|el| el.current_time() as f32).unwrap_or(0.0);
    let playing = STATE.with(|s| s.borrow().playing);
    (pos, playing)
}

pub fn active() -> bool {
    STATE.with(|s| s.borrow().playing)
}

pub fn begin_switch() {
    STATE.with(|s| s.borrow_mut().switching = true);
    stop();
}
pub fn finished() -> bool {
    STATE.with(|s| s.borrow().finished)
}
pub fn end_switch() {
    STATE.with(|s| s.borrow_mut().switching = false);
}
pub fn switching() -> bool {
    STATE.with(|s| s.borrow().switching)
}

pub fn set_volume(v: f32) {
    STATE.with(|s| s.borrow_mut().volume = v);
    if let Some(el) = audio() {
        el.set_volume(v.clamp(0.0, 1.0) as f64);
    }
}
pub fn get_volume() -> f32 {
    STATE.with(|s| s.borrow().volume)
}
