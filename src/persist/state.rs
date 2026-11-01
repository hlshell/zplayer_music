//! 配置持久化 + 缓存状态统计（供设置页展示与退出保存调用）。

use crate::persist::config::AppConfig;
use crate::AppWindow;
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

/// 计算缓存状态：占用 MB / 百分比 / 各类缓存文件数。
/// 内部做全目录递归遍历（total_usage / count_*），代价高——**必须**在阻塞线程/后台调用，
/// 严禁在 UI 线程直接跑（大缓存下会卡顿）。
/// 返回 (usage_mb, music, lyric, cover, percent)。
pub(crate) fn compute_cache_stats(limit_mb: i32) -> (i32, i32, i32, i32, f32) {
    let usage = crate::persist::cache::total_usage();
    let usage_mb = (usage / (1024 * 1024)) as i32;
    let pct = if limit_mb > 0 {
        (usage as f32) / (limit_mb as f32 * 1024.0 * 1024.0)
    } else {
        0.0
    };
    let music = crate::persist::cache::count_music() as i32;
    let lyric = crate::persist::cache::count_lyrics() as i32;
    let cover = crate::persist::cache::count_covers() as i32;
    (usage_mb, music, lyric, cover, pct.min(1.0))
}

/// 把缓存状态写入设置页 UI（仅设置属性，无任何 I/O，可安全在 UI 线程调用）。
pub(crate) fn set_cache_stats(u: &AppWindow, usage_mb: i32, music: i32, lyric: i32, cover: i32, pct: f32) {
    u.set_cache_usage_mb(usage_mb);
    u.set_cache_percent(pct.min(1.0));
    u.set_music_count(music);
    u.set_lyric_count(lyric);
    u.set_cover_count(cover);
}

/// 刷新设置页缓存状态（同步版）：占用 / 百分比 / 各类缓存文件数。
/// 内部为递归遍历，仅适用于缓存很小或可接受卡顿的场合；大缓存请改用 compute_cache_stats 后台算 + set_cache_stats 回填。
pub(crate) fn refresh_cache_stats(u: &AppWindow) {
    let limit = u.get_cache_limit_mb();
    let (m, mu, l, c, p) = compute_cache_stats(limit);
    set_cache_stats(u, m, mu, l, c, p);
}

/// 退出前把窗口位置 / 音量 / 播放进度写入共享配置并落盘。
pub(crate) fn save_state(ui: &AppWindow, shared: &Rc<RefCell<AppConfig>>) {
    let mut cfg = shared.borrow_mut();
    let _ = ui.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
        if let Ok(pos) = w.outer_position() {
            cfg.app.window_x = Some(pos.x);
            cfg.app.window_y = Some(pos.y);
        }
    });
    cfg.app.volume = crate::platform::audio::get_volume();
    // 当前有活动播放时记录进度（暂停也算当前位置；未播放不覆盖）
    let (p, playing) = crate::platform::audio::status();
    let cur = ui.get_current_song();
    if p > 0.0 || playing {
        cfg.last_song.duration_sec = cur.duration_sec;
        cfg.last_song.progress_sec = p * cur.duration_sec;
    }
    cfg.save();
    log::info!("[config] 配置已保存（窗口位置/音量/播放进度）");
}
