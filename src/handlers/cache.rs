//! 缓存管理回调：容量限制 / 缓存目录（封面/歌曲/根目录）/ 占用刷新 / 分类清理。
//!
//! 所有涉及全目录递归（total_usage / count_* / clear_*）的操作都**必须**放到
//! tokio 阻塞线程池（spawn_blocking）后台执行，再由 invoke_from_event_loop 回 UI 线程
//! 只做属性写入——否则大缓存下（数千封面+歌曲）会在 UI 线程同步卡死界面。

use super::Ctx;
use crate::persist::state;
use crate::AppWindow;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 后台计算缓存状态并回填 UI（阻塞线程算，UI 线程只写属性）。
fn refresh_stats_bg(rt_h: tokio::runtime::Handle, ui_weak: slint::Weak<AppWindow>, limit_mb: i32) {
    rt_h.spawn_blocking(move || {
        let (m, mu, l, c, p) = state::compute_cache_stats(limit_mb);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_weak.upgrade() {
                state::set_cache_stats(&u, m, mu, l, c, p);
            }
        });
    });
}

pub fn setup(ctx: &Ctx) {
    // 缓存容量限制（循环切换 0→1024→2048→5120→10240→0）
    // 切上限后立即按新值执行清理 + 重新统计，均后台做，避免 UI 卡顿。
    let ui = ctx.ui.clone();
    let shared_cl = ctx.shared.clone();
    let rt_cl = ctx.rt.clone();
    let ui_cl = ctx.ui.clone();
    ctx.app.clone_strong().on_cache_limit_clicked(move || {
        let Some(u) = ui.upgrade() else { return; };
        let next = match u.get_cache_limit_mb() {
            0 => 1024,
            1024 => 2048,
            2048 => 5120,
            5120 => 10240,
            _ => 0,
        };
        u.set_cache_limit_mb(next);
        shared_cl.borrow_mut().app.cache_limit_mb = next as u64;
        crate::persist::cache::init(&shared_cl.borrow());
        // 切上限立即按新值执行清理 + 重新统计，均在后台做（单次遍历），避免 UI 卡顿
        let rt_c = rt_cl.clone();
        let ui_c = ui_cl.clone();
        rt_c.spawn_blocking(move || {
            crate::persist::cache::enforce_limit_now();
            let (m, mu, l, c, p) = state::compute_cache_stats(next);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_c.upgrade() {
                    state::set_cache_stats(&u, m, mu, l, c, p);
                }
            });
        });
        log::info!("[cache] 容量限制切换为 {}MB", next);
    });

    // 封面缓存目录（原生文件夹对话框；web 无此能力，此 handler 整组仅非 wasm 编译）
    let ui = ctx.ui.clone();
    let shared_cd = ctx.shared.clone();
    let rt_cd = ctx.rt.clone();
    let ui_cd = ctx.ui.clone();
    ctx.app.clone_strong().on_cover_dir_clicked(move || {
        let Some(u) = ui.upgrade() else { return; };
        if let Some(s) = tinyfiledialogs::select_folder_dialog("选择封面缓存目录", "") {
            shared_cd.borrow_mut().cache.cover_dir = s.clone();
            crate::persist::cache::init(&shared_cd.borrow());
            u.set_cover_dir(s.clone().into());
            let limit = u.get_cache_limit_mb();
            refresh_stats_bg(rt_cd.clone(), ui_cd.clone(), limit);
            log::info!("[cache] 封面缓存目录 → {}", s);
        }
    });

    // 歌曲缓存目录
    let ui = ctx.ui.clone();
    let shared_sd = ctx.shared.clone();
    let rt_sd = ctx.rt.clone();
    let ui_sd = ctx.ui.clone();
    ctx.app.clone_strong().on_song_dir_clicked(move || {
        let Some(u) = ui.upgrade() else { return; };
        if let Some(s) = tinyfiledialogs::select_folder_dialog("选择歌曲缓存目录", "") {
            shared_sd.borrow_mut().cache.song_dir = s.clone();
            crate::persist::cache::init(&shared_sd.borrow());
            u.set_song_dir(s.clone().into());
            let limit = u.get_cache_limit_mb();
            refresh_stats_bg(rt_sd.clone(), ui_sd.clone(), limit);
            log::info!("[cache] 歌曲缓存目录 → {}", s);
        }
    });

    // 缓存占用刷新（进度条 / 文件数）：后台统计，避免手动刷新卡顿。
    let ui = ctx.ui.clone();
    let rt_rf = ctx.rt.clone();
    let ui_rf = ctx.ui.clone();
    ctx.app.clone_strong().on_cache_refresh(move || {
        if let Some(u) = ui.upgrade() {
            let limit = u.get_cache_limit_mb();
            refresh_stats_bg(rt_rf.clone(), ui_rf.clone(), limit);
        }
    });

    // 分类清理：0=音乐 1=歌词 2=封面 3=全部。
    // 整个清理在后台阻塞线程执行，UI 全程可操作；防重入用「Rust 侧 AtomicBool 门闩 + UI 侧
    // sys-clearing 禁用清理/刷新按钮」双重保障——清理进行中再点直接忽略，杜绝并发清理互删。
    let ui = ctx.ui.clone();
    let rt_clr = ctx.rt.clone();
    let ui_clr = ctx.ui.clone();
    let toast_clr = ctx.toast.clone();
    let gate = Arc::new(AtomicBool::new(false));
    let gate_cb = gate.clone();
    ctx.app.clone_strong().on_settings_clear_cache(move |k| {
        let Some(u) = ui.upgrade() else { return; };
        // 防重入门闩：清理进行中直接忽略后续点击（UI 按钮已禁用，此为兜底）
        if gate.swap(true, Ordering::SeqCst) {
            return;
        }
        u.set_sys_clearing(true);
        let rt_h = rt_clr.clone();
        let ui_weak = ui_clr.clone();
        let gate_g = gate_cb.clone();
        let toast_g = toast_clr.clone();
        let limit = u.get_cache_limit_mb();
        rt_h.spawn_blocking(move || {
            let n = match k {
                0 => crate::persist::cache::clear_music(),
                1 => crate::persist::cache::clear_lyrics(),
                2 => crate::persist::cache::clear_covers(),
                _ => crate::persist::cache::clear_all(),
            };
            crate::persist::cache::enforce_limit_now();
            // 后台统计（递归遍历），避免清理完成后的刷新再次卡 UI
            let (m, mu, l, c, p) = state::compute_cache_stats(limit);
            log::info!("[cache] 清理 k={} 删除 {} 个文件", k, n);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_weak.upgrade() {
                    state::set_cache_stats(&u, m, mu, l, c, p);
                    u.set_sys_clearing(false);
                    toast_g.show(&format!("已清理 {} 个文件", n), "ok");
                }
                // 无论窗口是否存活都解锁门闩，避免下次清理被永久挡住
                gate_g.store(false, Ordering::SeqCst);
            });
        });
    });
}
