//! 系统托盘（Windows/macOS/Linux）：常驻图标（启动即创建）+ 左键单击显隐切换 +
//! 右键 Slint 自绘菜单（ui/components/tray-menu.slint，独立置顶透明窗口）。
//! 事件接收用独立阻塞线程（全局 channel 的 recv 阻塞等待，零轮询开销），
//! 命中后 invoke_from_event_loop 转发 UI 线程处理（Slint 属性/回调仅限 UI 线程）。
//!
//! macOS 特殊性（tray-icon 文档）：托盘图标必须在主线程、且事件循环已运行后创建，
//! 否则全屏应用等场景会出问题 → init 统一经 invoke_from_event_loop 延迟到循环内执行。
//!
//! 托盘菜单窗口豁免 main.rs 全局窗口属性钩子（该钩子本为主窗口定制：
//! 记住位置/最小 900x560 等）：TrayMenu::new() 前置位 NEXT_IS_TRAY，
//! hook 内 take_tray_flag() 取走并复位 → 改用 apply_tray_attrs。hook 在
//! create_window_adapter（即 ::new()）内同步触发，时序确定无竞态。

use slint::ComponentHandle;
use slint::Model;
use slint::winit_030::WinitWindowAccessor;
use slint::winit_030::winit;
use std::sync::atomic::{AtomicBool, Ordering};

// 托盘实例必须活到进程结束（drop 即从托盘移除）。tray-icon 在 Windows 内部是 Rc（!Send/!Sync），
// 不能放 static OnceLock；托盘只在 UI 线程创建/使用，直接 mem::forget 泄漏常驻（同项目 Timer 惯例）。

static NEXT_IS_TRAY: AtomicBool = AtomicBool::new(false);

/// main.rs 窗口属性钩子调用：取走托盘标志（true → 本次窗口创建走 apply_tray_attrs）。
pub(crate) fn take_tray_flag() -> bool {
    NEXT_IS_TRAY.swap(false, Ordering::SeqCst)
}

/// 托盘菜单窗口属性：无边框/透明/置顶/不可缩放/创建时隐藏（show 时再显）。
pub(crate) fn apply_tray_attrs(
    attrs: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    let attrs = attrs
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
        .with_visible(false);
    // Windows 附加：跳任务栏（托盘菜单不该有任务栏按钮）
    #[cfg(target_os = "windows")]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_skip_taskbar(true)
    };
    attrs
}

// 菜单窗口单例（UI 线程）：mem::forget 常驻，Weak 供右键事件显示用
thread_local! {
    static MENU: std::cell::RefCell<Option<slint::Weak<crate::TrayMenu>>> =
        const { std::cell::RefCell::new(None) };
    static MENU_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 初始化托盘（在 UI 线程调用一次）：延迟到事件循环内创建图标 + 自绘菜单窗口。
/// app 用 Weak 避免循环强引用。
pub fn init(app: slint::Weak<crate::AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || init_on_ui(app));
}

fn init_on_ui(app: slint::Weak<crate::AppWindow>) {
    // 托盘图标：编译期嵌入 logo，缩到 32px（系统托盘标准尺寸）
    let icon = image::load_from_memory(include_bytes!("../../ui/assets/app-icon.png"))
        .map_err(|e| e.to_string())
        .and_then(|d| {
            let rgba = d.resize(32, 32, image::imageops::FilterType::Lanczos3).to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            tray_icon::Icon::from_rgba(rgba.into_raw(), w, h).map_err(|e| e.to_string())
        });
    let icon = match icon {
        Ok(i) => i,
        Err(e) => {
            log::warn!("[tray] 图标解码失败: {e}");
            return;
        }
    };

    // 不挂原生菜单：右键弹 Slint 自绘菜单（tray-menu.slint）
    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("ZPlayer Music")
        .with_icon(icon)
        // 左键不弹菜单：走单击事件做显示/隐藏切换
        .with_menu_on_left_click(false)
        .build();
    let tray = match tray {
        Ok(t) => t,
        Err(e) => {
            log::warn!("[tray] 创建失败: {e}");
            return;
        }
    };
    std::mem::forget(tray);
    log::info!("[tray] 已创建");

    // ---- 自绘菜单窗口（单例，创建即常驻）----
    NEXT_IS_TRAY.store(true, Ordering::SeqCst);
    let menu = crate::TrayMenu::new();
    NEXT_IS_TRAY.store(false, Ordering::SeqCst);
    let menu = match menu {
        Ok(m) => m,
        Err(e) => {
            log::warn!("[tray] 菜单窗口创建失败: {e}");
            return;
        }
    };
    wire_menu_callbacks(&menu, app.clone());
    // 预创建 winit 窗口：show+hide 让事件循环启动时建窗（隐藏态），
    // 首次右键时 with_winit_window 即可同步定位，避免首开定位闪烁
    let _ = menu.show();
    let _ = menu.hide();
    MENU.with(|m| *m.borrow_mut() = Some(menu.as_weak()));
    std::mem::forget(menu);

    // ---- 点击事件线程：左键 Up → 显隐切换；右键 Up → 弹自绘菜单（带物理坐标）----
    std::thread::spawn(move || {
        let rx = tray_icon::TrayIconEvent::receiver();
        while let Ok(ev) = rx.recv() {
            if let tray_icon::TrayIconEvent::Click {
                button,
                button_state: tray_icon::MouseButtonState::Up,
                position,
                ..
            } = ev
            {
                match button {
                    tray_icon::MouseButton::Left => forward(app.clone(), "toggle"),
                    tray_icon::MouseButton::Right => show_menu(app.clone(), position.x, position.y),
                    _ => {}
                }
            }
        }
    });
}

/// 菜单回调 → 转发到 AppWindow（与原有原生菜单动作语义一致）
fn wire_menu_callbacks(menu: &crate::TrayMenu, app: slint::Weak<crate::AppWindow>) {
    let a = app.clone();
    menu.on_action_show(move || {
        let Some(u) = a.upgrade() else { return; };
        let _ = u.window().with_winit_window(|w: &winit::window::Window| {
            w.set_visible(true);
            w.set_minimized(false);
            w.focus_window();
        });
    });
    let a = app.clone();
    menu.on_action_fav(move || {
        if let Some(u) = a.upgrade() {
            u.invoke_like_toggle();
        }
    });
    let a = app.clone();
    menu.on_action_prev(move || {
        if let Some(u) = a.upgrade() {
            u.invoke_prev_song();
        }
    });
    let a = app.clone();
    menu.on_action_play(move || {
        if let Some(u) = a.upgrade() {
            u.invoke_play_toggle();
        }
    });
    let a = app.clone();
    menu.on_action_next(move || {
        if let Some(u) = a.upgrade() {
            u.invoke_next_song();
        }
    });
    // 语言二级菜单：复用设置页语言切换（id=2，按索引；含配置持久化 + i18n 重载）
    let a = app.clone();
    menu.on_action_lang(move |idx| {
        if let Some(u) = a.upgrade() {
            u.invoke_settings_select_changed(2, idx);
        }
    });
    let a = app;
    menu.on_action_quit(move || {
        if let Some(u) = a.upgrade() {
            u.invoke_force_quit();
        }
    });
    let mw = menu.as_weak();
    menu.on_dismiss(move || hide_menu(&mw));
}

fn hide_menu(mw: &slint::Weak<crate::TrayMenu>) {
    if let Some(m) = mw.upgrade() {
        let _ = m.hide();
    }
    MENU_OPEN.with(|c| c.set(false));
}

/// 右键托盘图标 → 显示自绘菜单（物理像素坐标，UI 线程执行）
fn show_menu(app: slint::Weak<crate::AppWindow>, px: f64, py: f64) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(menu) = MENU.with(|m| m.borrow().as_ref().and_then(|w| w.upgrade())) else {
            return;
        };
        // toggle 语义：已开 → 关闭
        if MENU_OPEN.with(|c| c.get()) {
            hide_menu(&menu.as_weak());
            return;
        }
        let Some(u) = app.upgrade() else { return; };

        // ---- 状态同步（两窗口 global 独立，每次 show 前从主窗口拷贝）----
        let song = u.get_current_song();
        menu.set_song_title(if song.name.is_empty() {
            "ZPlayer Music".into()
        } else {
            song.name
        });
        menu.set_playing(u.get_playing());
        menu.set_liked(song.liked);
        menu.set_dark(u.get_dark());
        menu.set_theme_color_index(u.get_theme_color_index());
        let names: Vec<slint::SharedString> = u.get_language_options().iter().collect();
        menu.set_lang_names(std::rc::Rc::new(slint::VecModel::from(names)).into());
        menu.set_lang_index(u.get_language_index());
        menu.set_sub_open(false);
        let (src, dst) = (u.global::<crate::Language>(), menu.global::<crate::Language>());
        dst.set_tray_show(src.get_tray_show());
        dst.set_tray_fav(src.get_tray_fav());
        dst.set_tray_unfav(src.get_tray_unfav());
        dst.set_tray_prev(src.get_tray_prev());
        dst.set_tray_play(src.get_tray_play());
        dst.set_tray_pause(src.get_tray_pause());
        dst.set_tray_next(src.get_tray_next());
        dst.set_tray_language(src.get_tray_language());
        dst.set_tray_quit(src.get_tray_quit());

        // ---- 定位：窗口拉伸到点击点所在显示器全屏，菜单画在点击点 ----
        if !place_window(&menu, px, py) {
            // 首次打开：winit 窗口尚未创建（init 的预创建 race 兜底）。
            // 先 show 触发建窗，下一 tick 再定位（窗口透明，不可见错位帧）
            let _ = menu.show();
            MENU_OPEN.with(|c| c.set(true));
            let mw = menu.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(40), move || {
                if let Some(m) = mw.upgrade() {
                    place_window(&m, px, py);
                    m.invoke_refocus();
                }
            });
            return;
        }
        let _ = menu.show();
        MENU_OPEN.with(|c| c.set(true));
        menu.invoke_refocus();
    });
}

/// 把菜单窗口拉伸到 (px,py) 物理坐标所在显示器全屏，并写入逻辑坐标 click-x/y。
/// 窗口未创建时返回 false（调用方走延迟重试）。
fn place_window(menu: &crate::TrayMenu, px: f64, py: f64) -> bool {
    let mut done = false;
    let _ = menu.window().with_winit_window(|w: &winit::window::Window| {
        let scale = w.scale_factor();
        // 点击点所在显示器（物理坐标包含测试），取不到回退主屏/当前屏
        let mon = w
            .available_monitors()
            .find(|m| {
                let (p, s) = (m.position(), m.size());
                px >= p.x as f64
                    && px < (p.x + s.width as i32) as f64
                    && py >= p.y as f64
                    && py < (p.y + s.height as i32) as f64
            })
            .or_else(|| w.current_monitor())
            .or_else(|| w.primary_monitor());
        let Some(mon) = mon else { return };
        let (origin, size) = (mon.position(), mon.size());
        w.set_outer_position(winit::dpi::PhysicalPosition::new(origin.x, origin.y));
        let _ = w.request_inner_size(winit::dpi::PhysicalSize::new(size.width, size.height));
        // Windows：补 WS_EX_NOACTIVATE（不抢前台焦点；winit 0.30 无对应属性）
        #[cfg(target_os = "windows")]
        apply_no_activate(w);
        // 物理 → 逻辑（相对窗口原点）
        menu.set_click_x(((px - origin.x as f64) / scale) as f32);
        menu.set_click_y(((py - origin.y as f64) / scale) as f32);
        done = true;
    });
    done
}

/// Windows：给菜单窗口加 WS_EX_NOACTIVATE，避免点击时抢夺前台焦点
#[cfg(target_os = "windows")]
fn apply_no_activate(w: &winit::window::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::um::winuser::{GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE};
    let Ok(h) = w.window_handle() else { return };
    let RawWindowHandle::Win32(wh) = h.as_raw() else { return };
    let hwnd = wh.hwnd.get() as winapi::shared::windef::HWND;
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE as isize);
    }
}

/// 左键单击托盘图标：显示/隐藏切换（转发到 UI 线程）
fn forward(app: slint::Weak<crate::AppWindow>, action: &'static str) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(u) = app.upgrade() else { return; };
        if action == "toggle" {
            let _ = u.window().with_winit_window(|w: &winit::window::Window| {
                // winit 0.30 is_visible 返回 Option<bool>：未知时按"已隐藏"处理（点击应显示）
                let show = !w.is_visible().unwrap_or(false);
                w.set_visible(show);
                if show {
                    w.set_minimized(false);
                    w.focus_window();
                }
            });
        }
    });
}
