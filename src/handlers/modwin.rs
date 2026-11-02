//! 窗口外壳回调：窗口控制（最小化/最大化/关闭）、迷你窗口模式与播放列表抽屉、
//! 标题栏拖拽/双击、启动动画（splash 淡出）、跑马灯时钟。
//!
//! 注意：所有 slint::Timer 都必须存活到进程结束（drop 即停止）→ 用 mem::forget leak。

use super::Ctx;
use crate::persist::state;
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// 隐藏主窗口（“最小化到系统托盘”）而**不结束事件循环**。
///
/// 不能走 Slint 的 `Window::hide()`（含 `CloseRequestResponse::HideWindow` 的隐式隐藏）：
/// 那会执行 `WindowInner::hide()` → `context().release_keepalive()`，而本项目的托盘由
/// tray-icon crate 自建、不参与 Slint 的 keepalive 计数，主窗口是唯一计数 → 归零后
/// Slint 直接 `quit_event_loop()`，表现为“设置最小化到托盘，点关闭却直接退出”。
/// 这里直接操纵 winit 窗口可见性，Slint 侧 keepalive 保持不变，进程与音频继续常驻。
fn hide_main_window(u: &crate::AppWindow) {
    let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
        w.set_visible(false);
    });
    log::info!("[close] 窗口已隐藏到托盘（keepalive 保持不变）");
}

pub fn setup(ctx: &Ctx, saved_pos: Option<(i32, i32)>) {
    // ---- 窗口控制：最小化 / 最大化 / 关闭 ----
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_win_minimize(move || {
        if let Some(u) = ui_weak.upgrade() {
            u.window().set_minimized(true);
        }
    });
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_win_maximize(move || {
        if let Some(u) = ui_weak.upgrade() {
            let target = u
                .window()
                .with_winit_window(|w: &slint::winit_030::winit::window::Window| !w.is_maximized())
                .unwrap_or(false);
            u.window().set_maximized(target);
        }
    });
    let ui_weak = ctx.ui.clone();
    let shared_close = ctx.shared.clone();
    ctx.app.clone_strong().on_win_close(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        // 迷你窗口（画中画）：X 表示"恢复正常窗口"，不走关闭流程也不弹卡片
        if u.get_mini_mode() {
            u.invoke_win_mini();
            return;
        }
        // 先取出值再 match：match 的借用临时存活到整个 match 结束，
        // 直接在 match 分支里 save_state（内部 borrow_mut）会 panic "already borrowed"，
        // 瞬间崩溃 → 表现为"关闭行为设为最小化到托盘，点关闭却直接退出"。
        let behavior = shared_close.borrow().ui.close_behavior_index;
        match behavior {
            // 最小化到托盘：保存后隐藏窗口（托盘常驻）
            1 => {
                state::save_state(&u, &shared_close);
                hide_main_window(&u);
            }
            // 直接退出
            2 => {
                state::save_state(&u, &shared_close);
                crate::core::logger::mark_clean_exit();
                let _ = slint::quit_event_loop();
            }
            // 每次询问：应用内关闭卡片（不再弹原生窗口）
            _ => {
                u.set_close_remember(false);
                u.set_close_dialog_open(true);
            }
        }
    });
    // Alt+F4 / 任务栏关闭也会触发 close-requested：同样按关闭行为处理
    {
        let ui_weak = ctx.ui.clone();
        let shared_close = ctx.shared.clone();
        ctx.app.clone_strong().window().on_close_requested(move || {
            let Some(u) = ui_weak.upgrade() else { return slint::CloseRequestResponse::HideWindow; };
            // 迷你窗口：先恢复正常窗口，不关闭
            if u.get_mini_mode() {
                u.invoke_win_mini();
                return slint::CloseRequestResponse::KeepWindowShown;
            }
            // 同上：先把值拷出来，避免 match 期间一直持有 borrow 导致 save_state panic
            let behavior = shared_close.borrow().ui.close_behavior_index;
            match behavior {
                // 最小化到托盘：手工隐藏后返回 KeepWindowShown。
                // 返回 HideWindow 会让 Slint 执行 Window::hide()，从而 release_keepalive
                // 让窗口计数归零并 quit_event_loop（见 hide_main_window 注释）。
                1 => {
                    state::save_state(&u, &shared_close);
                    hide_main_window(&u);
                    slint::CloseRequestResponse::KeepWindowShown
                }
                // 直接退出：干净退出并关闭
                2 => {
                    state::save_state(&u, &shared_close);
                    crate::core::logger::mark_clean_exit();
                    let _ = slint::quit_event_loop();
                    slint::CloseRequestResponse::HideWindow
                }
                // 每次询问：保留窗口，弹出应用内关闭卡片
                _ => {
                    u.set_close_remember(false);
                    u.set_close_dialog_open(true);
                    slint::CloseRequestResponse::KeepWindowShown
                }
            }
        });
    }

    // ---- 关闭确认卡片：取消 / 退出应用 / 最小化到托盘 ----
    {
        let ui_weak = ctx.ui.clone();
        ctx.app.clone_strong().on_close_dialog_cancel(move || {
            if let Some(u) = ui_weak.upgrade() {
                u.set_close_dialog_open(false);
                u.set_close_remember(false);
            }
        });
    }
    {
        // 退出应用：勾选「记住我的选择」则把关闭行为固化为「直接退出」
        let ui_weak = ctx.ui.clone();
        let shared_close = ctx.shared.clone();
        ctx.app.clone_strong().on_close_dialog_exit(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            if u.get_close_remember() {
                let mut c = shared_close.borrow_mut();
                c.ui.close_behavior_index = 2;
                c.save();
                drop(c);
                u.set_close_behavior_index(2);
            }
            u.set_close_dialog_open(false);
            state::save_state(&u, &shared_close);
            crate::core::logger::mark_clean_exit();
            let _ = slint::quit_event_loop();
        });
    }
    {
        // 最小化到托盘：勾选「记住我的选择」则把关闭行为固化为「最小化到托盘」
        let ui_weak = ctx.ui.clone();
        let shared_close = ctx.shared.clone();
        ctx.app.clone_strong().on_close_dialog_tray(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            if u.get_close_remember() {
                let mut c = shared_close.borrow_mut();
                c.ui.close_behavior_index = 1;
                c.save();
                drop(c);
                u.set_close_behavior_index(1);
            }
            u.set_close_dialog_open(false);
            state::save_state(&u, &shared_close);
            hide_main_window(&u);
        });
    }

    // ---- 托盘菜单「退出」：无条件退出（保存状态 + 标记干净退出），不走关闭行为分支 ----
    {
        let ui_weak = ctx.ui.clone();
        let shared_quit = ctx.shared.clone();
        ctx.app.clone_strong().on_force_quit(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            state::save_state(&u, &shared_quit);
            crate::core::logger::mark_clean_exit();
            let _ = slint::quit_event_loop();
        });
    }

    // ---- 迷你窗口模式：进入缩到小窗 + 迷你 UI；退出恢复原尺寸/位置 ----
    {
        let ui_weak = ctx.ui.clone();
        let normal_rect: Rc<RefCell<Option<(i32, i32, i32, i32)>>> = Rc::new(RefCell::new(None));
        ctx.app.clone_strong().on_win_mini({
            let ui_weak = ui_weak.clone();
            let normal_rect = normal_rect.clone();
            move || {
                if let Some(u) = ui_weak.upgrade() {
                    let mini = u.get_mini_mode();
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
                        if !mini {
                            crate::platform::winshell::set_mini_queue(false);
                            u.set_mini_queue_open(false);
                            u.set_mini_mode(true);
                            if hwnd != 0 {
                                let r = crate::platform::winshell::get_outer_rect(hwnd);
                                *normal_rect.borrow_mut() = Some(r);
                                crate::platform::winshell::set_mini(true);
                                crate::platform::winshell::move_window(
                                    hwnd, r.0, r.1,
                                    (crate::platform::winshell::MINI_W * scale) as i32,
                                    (crate::platform::winshell::MINI_H * scale) as i32,
                                );
                            }
                            let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                                use slint::winit_030::winit::window::WindowLevel;
                                w.set_resizable(false);
                                w.set_window_level(WindowLevel::AlwaysOnTop);
                            });
                        } else {
                            crate::platform::winshell::set_mini(false);
                            if hwnd != 0 {
                                if let Some(r) = normal_rect.borrow_mut().take() {
                                    crate::platform::winshell::move_window(hwnd, r.0, r.1, r.2 - r.0, r.3 - r.1);
                                }
                            }
                            let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                                use slint::winit_030::winit::window::WindowLevel;
                                w.set_resizable(true);
                                w.set_window_level(WindowLevel::Normal);
                            });
                            u.set_mini_mode(false);
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    { u.set_mini_mode(!mini); }
                    // 强制立即重绘：MoveWindow resize 会重建 wgpu surface，
                    // 若不及时重绘，resize 空帧会被 OS 填充为透明（"先透明再显示"）
                    u.window().request_redraw();
                }
            }
        });
    }

    // ---- 迷你窗口播放列表抽屉：向下弹出 + 鼠标离开 2s 自动收起 ----
    let mini_q_monitor = Rc::new(slint::Timer::default());
    let mini_q_leaving = Rc::new(RefCell::new(std::time::Instant::now()));
    {
        let ui_weak = ctx.ui.clone();
        let toggle_guard = Rc::new(Cell::new(std::time::Instant::now() - Duration::from_secs(1)));
        let toggle_guard2 = toggle_guard.clone();
        let ui_weak_t = ui_weak.clone();
        let leaving_reset = mini_q_leaving.clone();
        ctx.app.clone_strong().on_mini_queue_toggle(move || {
            if toggle_guard2.get().elapsed() < Duration::from_millis(300) {
                return;
            }
            toggle_guard2.set(std::time::Instant::now());
            *leaving_reset.borrow_mut() = std::time::Instant::now();
            if let Some(u) = ui_weak_t.upgrade() {
                crate::platform::winshell::set_mini_drawer(&u, !u.get_mini_queue_open());
            }
        });
        let monitor = mini_q_monitor.clone();
        let leaving_since = mini_q_leaving.clone();
        let ls2 = leaving_since.clone();
        monitor.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
            if let Some(u) = ui_weak.upgrade() {
                // 非迷你模式：鼠标热区仅用于迷你抽屉自动收起，直接复位并跳过
                // Win32 调用（GetCursorPos/GetWindowRect），避免正常模式空转
                if !u.get_mini_mode() {
                    if u.get_mini_queue_hover() {
                        u.set_mini_queue_hover(false);
                    }
                    return;
                }
                let mut hover = false;
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    #[cfg(target_os = "windows")]
                    {
                        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
                        if let Ok(h) = w.window_handle() {
                            if let RawWindowHandle::Win32(wh) = h.as_raw() {
                                let hwnd = wh.hwnd.get() as isize;
                                let scale = w.scale_factor();
                                let mut pt = crate::platform::winshell::POINT::default();
                                if crate::platform::winshell::get_cursor_pos(&mut pt) {
                                    let (l, t, r, b) = crate::platform::winshell::get_outer_rect(hwnd);
                                    hover = pt.x >= l && pt.x < r && pt.y >= t && pt.y < b
                                        && (pt.y - t) as f64 > 76.0 * scale;
                                }
                            }
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    { let _ = w; }
                });
                u.set_mini_queue_hover(hover);
                let leaving = u.get_mini_mode() && u.get_mini_queue_open() && !hover;
                if leaving {
                    if ls2.borrow().elapsed() >= Duration::from_secs(2) {
                        crate::platform::winshell::set_mini_drawer(&u, false);
                    }
                } else {
                    *ls2.borrow_mut() = std::time::Instant::now();
                }
            }
        });
    }
    std::mem::forget(mini_q_monitor);
    std::mem::forget(mini_q_leaving);

    // ---- 自定义标题栏拖拽 / 双击最大化 ----
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_drag_start(move |_, _| {
        if let Some(u) = ui_weak.upgrade() {
            let _ = u.window().with_winit_window(|winit_window: &slint::winit_030::winit::window::Window| {
                winit_window.drag_window()
            });
        }
    });
    let ui_weak = ctx.ui.clone();
    let last_click = Rc::new(RefCell::new(Option::<std::time::Instant>::None));
    ctx.app.clone_strong().on_titlebar_clicked({
        let ui_weak = ui_weak.clone();
        let last_click = last_click.clone();
        move || {
            let now = std::time::Instant::now();
            let is_double = last_click
                .borrow()
                .map_or(false, |t| now.duration_since(t) < Duration::from_millis(400));
            if is_double {
                *last_click.borrow_mut() = None;
                if let Some(u) = ui_weak.upgrade() {
                    let target = u
                        .window()
                        .with_winit_window(|w: &slint::winit_030::winit::window::Window| !w.is_maximized())
                        .unwrap_or(false);
                    u.window().set_maximized(target);
                }
            } else {
                *last_click.borrow_mut() = Some(now);
            }
        }
    });

    // ---- 启动动画：驱动 splash-t / splash-loader，按时间线淡出 ----
    let ui_weak = ctx.ui.clone();
    let timer = Rc::new(RefCell::new(slint::Timer::default()));
    // 动画计时以"首次 tick（首帧真正渲染）"为基准，而非装配 handlers 的时间点——
    // 否则启动阶段任何同步慢操作（音频设备枚举/封面读盘等）会让 elapsed 在首帧就已偏大，
    // 导致 splash 内容未展开就被判定超时淡出（窗口先透明、splash 一闪而过）。
    let anim_start = Rc::new(Cell::new(std::time::Instant::now()));
    let anim_started = Rc::new(Cell::new(false));
    let timer_cb = timer.clone();
    #[cfg(target_os = "windows")]
    let corners_applied = Rc::new(Cell::new(false));
    #[cfg(not(target_os = "windows"))]
    let corners_applied = Rc::new(Cell::new(true));
    let pos_applied = Rc::new(Cell::new(false));
    let window_min = Rc::new(Cell::new(false));
    #[cfg(target_os = "windows")]
    let winmin_installed = Rc::new(Cell::new(false));
    let fade_at = Rc::new(Cell::new(0.0f32));
    #[cfg(target_os = "windows")]
    let winmin_cb = winmin_installed.clone();
    let corners_cb = corners_applied.clone();
    let pos_cb_applied = pos_applied.clone();
    let window_min_cb = window_min.clone();

    timer.borrow().start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            let elapsed = if !anim_started.get() {
                anim_started.set(true);
                anim_start.set(std::time::Instant::now());
                0.0
            } else {
                anim_start.get().elapsed().as_secs_f32()
            };
            if !pos_cb_applied.get() {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    pos_cb_applied.set(true);
                    // 无 saved_pos，或 saved_pos 已落在可视区外（迷你残留坐标/显示器变化/异常值）
                    // → 强制居中，避免窗口出现在屏幕外导致"启动后看不见窗口"
                    let need_center = match saved_pos {
                        None => true,
                        Some((x, y)) => match w.current_monitor() {
                            Some(m) => {
                                let mpos = m.position();
                                let msize = m.size();
                                let wsize = w.outer_size();
                                let off_x = x + (wsize.width as i32) < mpos.x || x > mpos.x + (msize.width as i32) - 40;
                                let off_y = y + (wsize.height as i32) < mpos.y || y > mpos.y + (msize.height as i32) - 40;
                                off_x || off_y
                            }
                            None => false,
                        },
                    };
                    if need_center {
                        if let Some(monitor) = w.current_monitor() {
                            let mpos = monitor.position();
                            let msize = monitor.size();
                            let wsize = w.outer_size();
                            let x = mpos.x + (msize.width as i32 - wsize.width as i32) / 2;
                            let y = mpos.y + (msize.height as i32 - wsize.height as i32) / 2;
                            w.set_outer_position(slint::winit_030::winit::dpi::PhysicalPosition::new(x, y));
                        }
                    }
                });
            }
            if false && !window_min_cb.get() {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    let scale = w.scale_factor();
                    w.set_min_inner_size(Some(slint::winit_030::winit::dpi::PhysicalSize::new(
                        (900.0 * scale) as u32,
                        (560.0 * scale) as u32,
                    )));
                    window_min_cb.set(true);
                });
            }
            #[cfg(target_os = "windows")]
            if !winmin_cb.get() {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle};
                    if let Ok(handle) = w.window_handle() {
                        if let RawWindowHandle::Win32(Win32WindowHandle { hwnd, .. }) = handle.as_raw() {
                            if crate::platform::winshell::install(hwnd.get() as *mut core::ffi::c_void) {
                                winmin_cb.set(true);
                            }
                            // 安装文件拖拽处理（本地视频播放器拖入视频文件播放）
                            let _ = crate::platform::drop_files::install(hwnd.get() as *mut core::ffi::c_void);
                        }
                    }
                });
            }
            #[cfg(target_os = "windows")]
            if !corners_cb.get() {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    if crate::platform::winshell::apply_rounded_corners(w) {
                        corners_cb.set(true);
                    }
                });
            }
            u.set_splash_t(elapsed);
            let loader = ((elapsed - 0.8) / 1.0).clamp(0.0, 1.0);
            u.set_splash_loader(loader);
            if fade_at.get() == 0.0 {
                let data_ready = !u.get_loading();
                if elapsed >= 2.4 || (data_ready && elapsed >= 1.6) {
                    u.set_splash_fade(true);
                    fade_at.set(elapsed);
                    log::info!("[boot] splash-fade {}ms", crate::boot_elapsed_ms());
                }
            } else if elapsed - fade_at.get() >= 0.9 {
                u.set_splash_visible(false);
                log::info!("[boot] splash-gone {}ms", crate::boot_elapsed_ms());
                timer_cb.borrow().stop();
            }
        },
    );
    std::mem::forget(timer);

    // ---- 跑马灯时钟：仅当有 MarqueeText 悬停时运行（闲置零开销）----
    let mq_timer = Rc::new(slint::Timer::default());
    let mq_ui = ctx.ui.clone();
    let timer = mq_timer.clone();
    ctx.app.clone_strong().global::<crate::MarqueeState>().on_hover_changed(move |on| {
        if !on {
            timer.stop();
            return;
        }
        let ui = mq_ui.clone();
        let mut pos: f32 = 0.0;
        let mut dir: f32 = 1.0;
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(30),
            move || {
                pos += dir * (0.030 / 3.0); // 3 秒单程
                if pos >= 1.0 { pos = 1.0; dir = -1.0; }
                if pos <= 0.0 { pos = 0.0; dir = 1.0; }
                if let Some(u) = ui.upgrade() {
                    u.global::<crate::MarqueeState>().set_pos(pos);
                }
            },
        );
    });
    std::mem::forget(mq_timer);

    // ---- 页面切换动画（方案A折中）：current-nav 变化 → 内容区淡出淡入 + 位移 ----
    // 由 Slint `changed current-nav` 事件驱动（见 appwindow.slint），无轮询开销。
    // 详情打开/关闭不切 current-nav，故不触发；详情展开期间忽略动画。
    {
        let ui_weak = ctx.ui.clone();
        ctx.app.clone_strong().on_nav_anim(move || {
            if let Some(u) = ui_weak.upgrade() {
                if u.get_detail_visible() {
                    return;
                }
                u.set_page_fade(false);
                u.set_page_shift(10.0);
                let uw = ui_weak.clone();
                slint::Timer::single_shot(std::time::Duration::from_millis(240), move || {
                    if let Some(u) = uw.upgrade() {
                        u.set_page_fade(true);
                        u.set_page_shift(0.0);
                    }
                });
            }
        });
    }
}
