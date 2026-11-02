//! 系统托盘（Windows）：常驻图标（启动即创建）+ 左键单击显示/隐藏切换 + 右键菜单
//! （显示主窗口 / 上一首 / 播放暂停 / 下一首 / 退出）。
//! 事件接收用独立阻塞线程（全局 channel 的 recv 阻塞等待，零轮询开销），
//! 命中后 invoke_from_event_loop 转发 UI 线程处理（Slint 属性/回调仅限 UI 线程）。

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

// 托盘实例必须活到进程结束（drop 即从托盘移除）。tray-icon 在 Windows 内部是 Rc（!Send/!Sync），
// 不能放 static OnceLock；托盘只在 UI 线程创建/使用，直接 mem::forget 泄漏常驻（同项目 Timer 惯例）。

/// 初始化托盘（在 UI 线程调用一次）：创建图标 + 菜单，启动事件转发线程。
/// app 用 Weak 避免循环强引用；lang 决定菜单文案（"zh"/"en"）。
pub fn init(app: slint::Weak<crate::AppWindow>, lang: &str) {
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

    // 右键菜单（文案跟随界面语言）
    let (t_show, t_prev, t_play, t_next, t_quit) = if lang == "en" {
        ("Show Window", "Previous", "Play/Pause", "Next", "Quit")
    } else {
        ("显示主窗口", "上一首", "播放/暂停", "下一首", "退出")
    };
    let menu = tray_icon::menu::Menu::new();
    let mi_show = tray_icon::menu::MenuItem::new(t_show, true, None);
    let mi_prev = tray_icon::menu::MenuItem::new(t_prev, true, None);
    let mi_play = tray_icon::menu::MenuItem::new(t_play, true, None);
    let mi_next = tray_icon::menu::MenuItem::new(t_next, true, None);
    let sep = tray_icon::menu::PredefinedMenuItem::separator();
    let mi_quit = tray_icon::menu::MenuItem::new(t_quit, true, None);
    if menu.append_items(&[&mi_show, &mi_prev, &mi_play, &mi_next, &sep, &mi_quit]).is_err() {
        log::warn!("[tray] 菜单构建失败");
        return;
    }
    // 菜单事件只回传 MenuId，这里建立 id → 动作映射
    let ids: Vec<(tray_icon::menu::MenuId, &'static str)> = vec![
        (mi_show.id().clone(), "show"),
        (mi_prev.id().clone(), "prev"),
        (mi_play.id().clone(), "play"),
        (mi_next.id().clone(), "next"),
        (mi_quit.id().clone(), "quit"),
    ];

    let tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("ZPlayer Music")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        // 左键不弹菜单：走单击事件做显示/隐藏切换（菜单仅右键弹出）
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

    // ---- 菜单事件线程：阻塞 recv，零轮询 ----
    let app_menu = app.clone();
    std::thread::spawn(move || {
        let rx = tray_icon::menu::MenuEvent::receiver();
        while let Ok(ev) = rx.recv() {
            if let Some((_, action)) = ids.iter().find(|(id, _)| id == &ev.id()) {
                forward(app_menu.clone(), action);
            }
        }
    });

    // ---- 左键单击线程：Left + Up 触发显示/隐藏切换 ----
    let app_click = app;
    std::thread::spawn(move || {
        let rx = tray_icon::TrayIconEvent::receiver();
        while let Ok(ev) = rx.recv() {
            if let tray_icon::TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = ev
            {
                forward(app_click.clone(), "toggle");
            }
        }
    });
}

/// 转发动作到 UI 线程执行（Slint 属性/回调仅限 UI 线程）
fn forward(app: slint::Weak<crate::AppWindow>, action: &'static str) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(u) = app.upgrade() else { return; };
        match action {
            // 菜单「显示主窗口」：强制显示并抢到前台（取消最小化 + 聚焦）
            "show" => {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    w.set_visible(true);
                    w.set_minimized(false);
                    w.focus_window();
                });
            }
            // 左键单击托盘图标：显示/隐藏切换。可见性直接问 winit 窗口真实状态——
            // modwin 隐藏窗口走的就是 winit set_visible，与 Slint 侧状态可能不同步
            "toggle" => {
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    // winit 0.30 is_visible 返回 Option<bool>：未知时按"已隐藏"处理（点击应显示）
                    let show = !w.is_visible().unwrap_or(false);
                    w.set_visible(show);
                    if show {
                        w.set_minimized(false);
                        w.focus_window();
                    }
                });
            }
            "prev" => u.invoke_prev_song(),
            "play" => u.invoke_play_toggle(),
            "next" => u.invoke_next_song(),
            // 菜单「退出」：直接保存+标记干净退出+退出，不触发关闭行为分支
            "quit" => u.invoke_force_quit(),
            _ => {}
        }
    });
}
