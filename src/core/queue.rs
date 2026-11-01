//! 队列播放逻辑：按播放模式挑上一首/下一首目标、播放队列指定项、记录上次播放歌曲。

use crate::persist::config::AppConfig;
use crate::persist::liked::{is_liked, sync_liked};
use crate::AppWindow;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;

/// 按播放模式从队列中挑选上一首/下一首目标索引。
/// mode: 0=循环播放(按序绕回) 1=单曲循环(重播当前) 2=随机播放(≠当前曲)。
pub(crate) fn pick_queue_target(u: &AppWindow, mode: i32, dir: i32) -> Option<usize> {
    let n = u.get_queue().row_count();
    if n == 0 { return None; }
    let cur = u.get_current_song();
    let mut idx = 0usize;
    for i in 0..n {
        if let Some(s) = u.get_queue().row_data(i) {
            if s.id == cur.id { idx = i; break; }
        }
    }
    if n == 1 { return Some(idx); }
    match mode {
        1 => Some(idx), // 单曲循环：重播当前
        2 => { // 随机播放：避免选到当前曲
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos()).unwrap_or(0) as usize;
            let mut r = (seed ^ (idx.wrapping_mul(2654435761))) % n;
            if r == idx { r = (r + 1) % n; }
            Some(r)
        }
        _ => { // 列表循环：按序前进/后退并绕回
            let next = (((idx as i32 + dir) % n as i32) + n as i32) % n as i32;
            Some(next as usize)
        }
    }
}

/// 播放队列中第 idx 首（设置当前曲目并触发解析播放，从头开始）。
/// 同时记录为"上次播放歌曲"（含封面 URL，供下次启动同步恢复）。
pub(crate) fn play_queue_index<F: Fn(i64, String, String, f32, f32) + ?Sized>(
    u: &AppWindow,
    idx: usize,
    shared: &Rc<RefCell<AppConfig>>,
    resolver: &F,
) {
    if let Some(s) = u.get_queue().row_data(idx) {
        let dur_str = format!("{:02}:{:02}", (s.duration_sec as u64) / 60, (s.duration_sec as u64) % 60);
        let song = crate::Song {
            id: s.id, name: s.name.clone(), artist: s.artist.clone(), album: s.album.clone(),
            cover: s.cover.clone(), duration: dur_str.into(), duration_sec: s.duration_sec,
            url: s.url.clone(),
            liked: is_liked(s.id as u32 as i64),
        };
        u.set_current_song(song);
        sync_liked(u);
        // 记录上次播放（封面 URL 随队列项写入，下次启动从磁盘缓存恢复封面）
        record_last_song(shared, u, s.id as u32 as i64);
        // 点击立即停旧流并归零进度（即刻反馈；目标曲多已被后台预取，缓存命中近乎立即出声）
        crate::platform::audio::stop();
        crate::platform::audio::begin_switch();
        u.set_progress(0.0);
        u.set_playing(true);
        // Slint int 是 i32：大 ID（>i32::MAX）存储为负数，必须经 u32 往返还原真实
        // 网易云 ID，否则解析/缓存键全错（上一首/下一首播不出的根因）
        resolver(s.id as u32 as i64, s.name.to_string(), s.artist.to_string(), s.duration_sec, 0.0);
    }
}

/// 把 AppWindow 的当前曲目记录到共享配置的 last_song（含封面来源 URL）。
/// 须在 set_current_song 之后调用；进度由 5s 节流/退出保存覆盖。
pub(crate) fn record_last_song(shared: &Rc<RefCell<AppConfig>>, u: &AppWindow, uid: i64) {
    let cur = u.get_current_song();
    shared.borrow_mut().last_song = crate::persist::config::LastSongSection {
        id: uid,
        name: cur.name.to_string(),
        artist: cur.artist.to_string(),
        album: cur.album.to_string(),
        duration_sec: cur.duration_sec,
        progress_sec: 0.0,
        cover_url: cur.url.to_string(),
    };
}
