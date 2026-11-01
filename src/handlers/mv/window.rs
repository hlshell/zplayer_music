//! MV 播放器窗口控制：全屏切换 + 音频互斥。
//! Esc 退出由 mv_player.slint 的 FocusScope 拦截后回调 mv_back 处理。

use crate::AppWindow;
use slint::winit_030::winit::window::Fullscreen;
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;

/// 切换窗口全屏（有边框全屏 ↔ 普通窗口）。切换后把状态写回 UI（最大化/还原图标切换）。
pub fn toggle_fullscreen(ui: &slint::Weak<AppWindow>) {
    let Some(u) = ui.upgrade() else { return };
    let mut now_fs = false;
    let _ = u.window().with_winit_window(
        |win: &slint::winit_030::winit::window::Window| {
            if win.fullscreen().is_some() {
                win.set_fullscreen(None);
                now_fs = false;
            } else {
                win.set_fullscreen(Some(Fullscreen::Borderless(None)));
                now_fs = true;
            }
        },
    );
    u.set_mv_fullscreen(now_fs);
}

/// 退出全屏（若当前处于全屏）：关闭 MV 时恢复普通窗口，避免主界面残留全屏。
pub fn exit_fullscreen(ui: &slint::Weak<AppWindow>) {
    let Some(u) = ui.upgrade() else { return };
    let _ = u.window().with_winit_window(
        |win: &slint::winit_030::winit::window::Window| {
            if win.fullscreen().is_some() {
                win.set_fullscreen(None);
            }
        },
    );
}

// ---- MV 画中画（compact）模式 ----
// 进入时记忆的普通窗口矩形（left, top, right, bottom），退出时还原。
thread_local! {
    static COMPACT_SAVED_RECT: std::cell::RefCell<Option<(i32, i32, i32, i32)>> =
        const { std::cell::RefCell::new(None) };
}

/// 切换画中画模式。
pub fn toggle_compact(ui: &slint::Weak<AppWindow>) {
    let Some(u) = ui.upgrade() else { return };
    if u.get_mv_compact() {
        exit_compact(ui);
    } else {
        enter_compact(ui);
    }
}

/// 进入画中画：退出全屏 → 记忆窗口矩形 → 缩到 480x270 → 置顶（保持可缩放）。
/// MV 引擎不停（解码/音频继续），仅窗口形态与 UI 精简变化，还原后无缝继续。
pub fn enter_compact(ui: &slint::Weak<AppWindow>) {
    let Some(u) = ui.upgrade() else { return };
    // 全屏态先退出：winit 全屏下 resize 无效
    exit_fullscreen(ui);
    // UI 属性先切（min-width 约束随之放低），对齐 modwin 的迷你模式顺序
    u.set_mv_fullscreen(false);
    u.set_mv_compact(true);
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let (hwnd, scale) = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
            let hwnd = w.window_handle()
                .map(|h| {
                    if let RawWindowHandle::Win32(wh) = h.as_raw() {
                        Some(wh.hwnd.get() as isize)
                    } else { None }
                })
                .ok().flatten().unwrap_or(0);
            (hwnd, w.scale_factor())
        }).unwrap_or((0, 1.0));
        if hwnd != 0 {
            let r = crate::platform::winshell::get_outer_rect(hwnd);
            COMPACT_SAVED_RECT.with(|c| *c.borrow_mut() = Some(r));
            crate::platform::winshell::set_mv_compact(true);
            // 位置保持在原窗口左上角，尺寸缩到 480x270
            crate::platform::winshell::move_window(
                hwnd, r.0, r.1,
                (crate::platform::winshell::MV_COMPACT_MIN_W * scale) as i32,
                (crate::platform::winshell::MV_COMPACT_MIN_H * scale) as i32,
            );
        }
        let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
            use slint::winit_030::winit::window::WindowLevel;
            // 保持可缩放（winmin 钳最小 480x270），置顶浮于其他窗口
            w.set_resizable(true);
            w.set_window_level(WindowLevel::AlwaysOnTop);
        });
        // 强制立即重绘：MoveWindow resize 会重建 wgpu surface，
        // 不及时重绘时 resize 空帧被 OS 填透明（"先透明再显示"闪烁）
        u.window().request_redraw();
    }
}

/// 退出画中画：还原记忆的窗口矩形与位置，取消置顶。
pub fn exit_compact(ui: &slint::Weak<AppWindow>) {
    let Some(u) = ui.upgrade() else { return };
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        crate::platform::winshell::set_mv_compact(false);
        let hwnd = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
            w.window_handle()
                .map(|h| {
                    if let RawWindowHandle::Win32(wh) = h.as_raw() {
                        Some(wh.hwnd.get() as isize)
                    } else { None }
                })
                .ok().flatten().unwrap_or(0)
        }).unwrap_or(0);
        if hwnd != 0 {
            if let Some(r) = COMPACT_SAVED_RECT.with(|c| c.borrow_mut().take()) {
                crate::platform::winshell::move_window(hwnd, r.0, r.1, r.2 - r.0, r.3 - r.1);
            }
        }
        let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
            use slint::winit_030::winit::window::WindowLevel;
            w.set_resizable(true);
            w.set_window_level(WindowLevel::Normal);
        });
        u.window().request_redraw();
    }
    u.set_mv_compact(false);
}

/// 打开 MV 播放器时暂停音乐播放（音频互斥：避免 MV 声音与音乐叠加）。
/// 注：退出 MV 时不自动恢复 rodio，由用户手动点 PlayBar 恢复。
pub fn pause_rodio() {
    let (_pos, playing) = crate::platform::audio::status();
    if playing {
        crate::platform::audio::toggle();
    }
}
