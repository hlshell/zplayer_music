//! 桌面歌词悬浮窗（独立置顶 / 透明 / 无边框）。
//!
//! 复用托盘菜单窗口的同一套「第二个 Slint 窗口」机制：
//!   `DesktopLyrics::new()` 前置位 NEXT_IS_DESKTOP_LYRICS，main.rs 的
//!   `with_winit_window_attributes_hook` 内 `take_desktop_lyrics_flag()` 取走并改用
//!   `apply_desktop_lyrics_attrs`（无边框/透明/置顶/不可缩放/创建时隐藏）。
//!
//! 交互：
//!   - 非锁定态可拖动整窗（drag-start → winit drag_window）；
//!   - 「锁定」→ `set_cursor_hittest(false)` 鼠标穿透（解锁入口在设置页/托盘菜单）；
//!   - 「×」→ 隐藏并回写主窗口开关。
//!
//! 歌词文本由 handlers/player/play.rs 的 250ms 进度轮询推送（update_line）。

use slint::winit_030::winit;
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

/// 窗口逻辑尺寸（Slint px）。高度与迷你模式一致（winshell::MINI_H = 76）。
const WIN_W: f32 = 760.0;
const WIN_H: f32 = 76.0;

/// 已唱 / 未唱配色预设数量（须与 ui/desktop/components/lyrics-ink.slint 的数组长度一致）。
const SUNG_COUNT: i32 = 9;
const UNSUNG_COUNT: i32 = 7;

/// 平滑推进定时器间隔：播放进度 250ms 才回调一次，中间按 30ms 插值补间，
/// 否则进度条会「跳一下停一下」，即肉眼可见的卡顿。
const SMOOTH_TICK_MS: u64 = 30;
/// 插值最大外推时长：暂停时位置不再变化，超过该值即停止外推，避免跑飞。
const SMOOTH_MAX_EXTRA_MS: i32 = 300;

static NEXT_IS_DESKTOP_LYRICS: AtomicBool = AtomicBool::new(false);

/// main.rs 窗口属性钩子调用：取走桌面歌词标志（true → 本次窗口创建走 apply_desktop_lyrics_attrs）。
pub(crate) fn take_desktop_lyrics_flag() -> bool {
    NEXT_IS_DESKTOP_LYRICS.swap(false, Ordering::SeqCst)
}

/// 桌面歌词窗口属性：无边框/透明/置顶/不可缩放/创建时隐藏（show 时再显）。
pub(crate) fn apply_desktop_lyrics_attrs(
    attrs: winit::window::WindowAttributes,
) -> winit::window::WindowAttributes {
    let attrs = attrs
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
        .with_visible(false);
    // Windows 附加：跳任务栏（悬浮歌词不应有任务栏按钮）
    #[cfg(target_os = "windows")]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows;
        attrs.with_skip_taskbar(true)
    };
    attrs
}

thread_local! {
    /// 窗口单例（UI 线程）：mem::forget 常驻，Weak 供进度推送/开关调用。
    static WIN: RefCell<Option<slint::Weak<crate::DesktopLyrics>>> = const { RefCell::new(None) };
    /// 是否已做过首次定位（用户拖动后不应被 show 重置）。
    static PLACED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// 字号(px) / 字体索引 / 已唱配色索引 / 未唱配色索引 / 描边开关
    /// 启动时由配置写入，悬浮窗内或设置页调整后同步更新。
    static STYLE: std::cell::Cell<(i32, i32, i32, i32, bool)> =
        const { std::cell::Cell::new((24, 0, 0, 0, true)) };
    /// 锁定态悬停轮询定时器（常驻）。
    static HOVER_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    /// 平滑推进锚点：(播放位置毫秒, 该位置被写入的时刻)。
    /// 每次 250ms 进度回调刷新；30ms 定时器按 elapsed 外推。
    static ANCHOR: RefCell<Option<(i32, std::time::Instant)>> = const { RefCell::new(None) };
    /// 平滑推进定时器（常驻）。
    static SMOOTH_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

fn upgrade_win() -> Option<crate::DesktopLyrics> {
    WIN.with(|w| w.borrow().as_ref().and_then(|x| x.upgrade()))
}

/// 启动恢复（事件循环内执行）：enabled=true 时创建窗口并显示，随后应用锁定状态。
pub fn init(
    app: slint::Weak<crate::AppWindow>,
    enabled: bool,
    locked: bool,
    font_size: i32,
    font_index: i32,
    sung_index: i32,
    unsung_index: i32,
    outline: bool,
) {
    STYLE.with(|f| {
        f.set((
            font_size.clamp(14, 44),
            font_index.rem_euclid(4),
            sung_index.rem_euclid(SUNG_COUNT),
            unsung_index.rem_euclid(UNSUNG_COUNT),
            outline,
        ))
    });
    let _ = slint::invoke_from_event_loop(move || {
        if !enabled {
            return;
        }
        if ensure_created(&app) {
            show();
            set_locked(locked);
        }
    });
}

/// 设置页开关：开 → 建窗并显示；关 → 隐藏。
/// 在 UI 线程调用（设置回调内）。
pub fn set_enabled(app: slint::Weak<crate::AppWindow>, on: bool) {
    if on {
        if ensure_created(&app) {
            show();
            // 窗口可能因上次「×」关闭时残留锁定态，按主窗口开关重新同步（穿透/按钮显隐）
            let locked = app
                .upgrade()
                .map(|u| u.get_desktop_lyrics_lock())
                .unwrap_or(false);
            set_locked(locked);
        }
    } else {
        hide();
        set_locked(false);
    }
}

/// 显示悬浮窗；首次显示时定位到主屏底部居中（距底约 110px）。
pub fn show() {
    let Some(win) = upgrade_win() else { return };
    let _ = win.show();
    if !PLACED.with(|c| c.replace(true)) {
        let _ = win
            .window()
            .with_winit_window(|w: &winit::window::Window| place_bottom_center(w));
    }
}

pub fn hide() {
    if let Some(win) = upgrade_win() {
        let _ = win.hide();
    }
}

/// 设置锁定态：锁定 = 背景透明 + 禁止拖动 + 鼠标穿透（悬停时临时恢复命中以显示解锁按钮）。
pub fn set_locked(locked: bool) {
    if let Some(win) = upgrade_win() {
        if win.get_locked() != locked {
            win.set_locked(locked);
        }
        if !locked {
            win.set_hover_controls(false);
        }
        let _ = win.window().with_winit_window(|w: &winit::window::Window| {
            let _ = w.set_cursor_hittest(!locked);
        });
    }
}

/// 设置页配色菜单：立即应用已唱 / 未唱预设（持久化由 handlers/settings 负责）。
pub fn set_colors(sung_index: i32, unsung_index: i32) {
    let s = sung_index.rem_euclid(SUNG_COUNT);
    let u = unsung_index.rem_euclid(UNSUNG_COUNT);
    STYLE.with(|f| {
        let (px, fi, _, _, outline) = f.get();
        f.set((px, fi, s, u, outline));
    });
    if let Some(win) = upgrade_win() {
        win.set_sung_index(s);
        win.set_unsung_index(u);
    }
}

/// 设置页描边开关：立即应用，无需重建悬浮窗。
pub fn set_outline(outline: bool) {
    STYLE.with(|f| {
        let (px, fi, s, u, _) = f.get();
        f.set((px, fi, s, u, outline));
    });
    if let Some(win) = upgrade_win() {
        win.set_outline(outline);
    }
}

/// 轮询鼠标屏幕坐标驱动控制条显隐：悬停进入窗口区域即显示控制条，
/// 锁定态下同时恢复/关闭命中（穿透）。显隐不依赖 TouchArea.has-hover，
/// 避免鼠标移到按钮上时控制条消失、点击落空。
fn start_hover_poll(win: &crate::DesktopLyrics) {
    if HOVER_TIMER.with(|t| t.borrow().is_some()) {
        return;
    }
    let weak = win.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(100),
        move || {
            let Some(w) = weak.upgrade() else { return };
            let inside = cursor_inside(&w);
            let locked = w.get_locked();
            if inside != w.get_hover_controls() {
                w.set_hover_controls(inside);
            }
            let hittest = if locked { inside } else { true };
            let _ = w
                .window()
                .with_winit_window(|nw: &winit::window::Window| {
                    let _ = nw.set_cursor_hittest(hittest);
                });
        },
    );
    HOVER_TIMER.with(|t| *t.borrow_mut() = Some(timer));
}

/// 播放位置平滑推进：250ms 的真实进度之间按 30ms 外推，
/// 让步进式回调变成连续增长，消除已唱区域的卡顿感。
fn start_smooth(win: &crate::DesktopLyrics) {
    if SMOOTH_TIMER.with(|t| t.borrow().is_some()) {
        return;
    }
    let weak = win.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(SMOOTH_TICK_MS),
        move || {
            let Some(w) = weak.upgrade() else { return };
            let Some((base, at)) = ANCHOR.with(|a| *a.borrow()) else { return };
            let extra = (at.elapsed().as_millis() as i32).min(SMOOTH_MAX_EXTRA_MS);
            let mut est = base.saturating_add(extra);
            // 不越过当前行结尾：最后一段停在满进度等下一行回调
            let end = w.get_end_ms();
            if end > base && est > end {
                est = end;
            }
            if est != w.get_pos_ms() {
                w.set_pos_ms(est);
            }
        },
    );
    SMOOTH_TIMER.with(|t| *t.borrow_mut() = Some(timer));
}

/// 鼠标是否位于悬浮窗内外框内（物理坐标）。
#[cfg(target_os = "windows")]
fn cursor_inside(win: &crate::DesktopLyrics) -> bool {
    let mut pt = crate::platform::winshell::POINT::default();
    if !crate::platform::winshell::get_cursor_pos(&mut pt) {
        return false;
    }
    let mut inside = false;
    let _ = win
        .window()
        .with_winit_window(|w: &winit::window::Window| {
            if let Ok(pos) = w.outer_position() {
                let size = w.inner_size();
                inside = pt.x >= pos.x
                    && pt.x < pos.x + size.width as i32
                    && pt.y >= pos.y
                    && pt.y < pos.y + size.height as i32;
            }
        });
    inside
}

#[cfg(not(target_os = "windows"))]
fn cursor_inside(_win: &crate::DesktopLyrics) -> bool {
    false
}

/// 推送当前行歌词与进度（250ms 一次）。先判再写，避免无谓重绘。
pub fn update_line(song: &str, text: &str, start_ms: i32, end_ms: i32, pos_ms: i32) {
    let Some(win) = upgrade_win() else { return };
    // 每次真实回调都重置平滑锚点（即使位置未变），30ms 定时器从这里继续外推
    ANCHOR.with(|a| *a.borrow_mut() = Some((pos_ms, std::time::Instant::now())));
    if win.get_song().as_str() != song {
        win.set_song(song.into());
    }
    if win.get_end_ms() != end_ms {
        win.set_end_ms(end_ms);
    }
    if win.get_pos_ms() != pos_ms {
        win.set_pos_ms(pos_ms);
    }
    if win.get_text().as_str() != text {
        win.set_text(text.into());
    }
    // start-ms 最后写入：它会触发填充层换实例，此时文本、行尾和进度均已是新行数据，
    // 避免新实例先按旧行尾算出 100% 再向当前进度反向补间。
    if win.get_start_ms() != start_ms {
        win.set_start_ms(start_ms);
    }
}

/// 切歌清空当前行（保留歌名）。
pub fn reset() {
    ANCHOR.with(|a| *a.borrow_mut() = None);
    let Some(win) = upgrade_win() else { return };
    win.set_end_ms(0);
    win.set_pos_ms(0);
    if !win.get_text().is_empty() {
        win.set_text("".into());
    }
    win.set_start_ms(0);
}

/// 创建窗口（幂等），返回是否可用。
fn ensure_created(app: &slint::Weak<crate::AppWindow>) -> bool {
    if upgrade_win().is_some() {
        return true;
    }
    NEXT_IS_DESKTOP_LYRICS.store(true, Ordering::SeqCst);
    let created = crate::DesktopLyrics::new();
    NEXT_IS_DESKTOP_LYRICS.store(false, Ordering::SeqCst);
    let win = match created {
        Ok(w) => w,
        Err(e) => {
            log::warn!("[desktop-lyrics] 窗口创建失败: {e}");
            return false;
        }
    };

    // 预创建 winit 窗口（show+hide），随后设置固定尺寸
    let _ = win.show();
    let _ = win.hide();
    let _ = win.window().with_winit_window(|w: &winit::window::Window| {
        let _ = w.request_inner_size(winit::dpi::LogicalSize::new(WIN_W, WIN_H));
    });
    // 同步 Slint 侧窗口尺寸（仅 request_inner_size 可能被 Slint 布局尺寸覆盖，导致高度不生效）
    win.window()
        .set_size(slint::LogicalSize::new(WIN_W, WIN_H));

    // 恢复字号 / 字体风格 / 已唱与未唱配色 / 描边
    let (px, fi, si, ui, outline) = STYLE.with(|f| f.get());
    win.set_font_size_px(px);
    win.set_font_index(fi);
    win.set_sung_index(si);
    win.set_unsung_index(ui);
    win.set_outline(outline);
    start_hover_poll(&win);
    start_smooth(&win);

    // 拖动整窗
    let ww = win.as_weak();
    win.on_drag_start(move |_, _| {
        if let Some(w) = ww.upgrade() {
            let _ = w
                .window()
                .with_winit_window(|w: &winit::window::Window| w.drag_window());
        }
    });

    // 锁定 / 解锁切换：写窗口态 + 回写主窗口设置（持久化由 handlers/settings 负责）
    let app_l = app.clone();
    let ww = win.as_weak();
    win.on_lock_clicked(move || {
        let next = ww.upgrade().map(|w| !w.get_locked()).unwrap_or(false);
        set_locked(next);
        if let Some(u) = app_l.upgrade() {
            u.set_desktop_lyrics_lock(next);
            u.invoke_desktop_lyrics_lock_changed(next);
        }
    });

    // 字号 / 字体风格调整：立即生效并回写主窗口持久化
    let app_f = app.clone();
    let ww = win.as_weak();
    win.on_font_changed(move |px, fi| {
        let px = px.clamp(14, 44);
        let fi = fi.rem_euclid(4);
        STYLE.with(|f| {
            let (_, _, si, ui, outline) = f.get();
            f.set((px, fi, si, ui, outline));
        });
        if let Some(w) = ww.upgrade() {
            w.set_font_size_px(px);
            w.set_font_index(fi);
        }
        if let Some(u) = app_f.upgrade() {
            u.invoke_desktop_lyrics_font_changed(px, fi);
        }
    });

    // 控制条配色按钮：循环切换已唱预设，立即生效并回写主窗口持久化
    let app_k = app.clone();
    win.on_color_changed(move |si, ui| {
        let si = si.rem_euclid(SUNG_COUNT);
        let ui = ui.rem_euclid(UNSUNG_COUNT);
        set_colors(si, ui);
        if let Some(u) = app_k.upgrade() {
            u.set_desktop_lyrics_sung(si);
            u.set_desktop_lyrics_unsung(ui);
            u.invoke_desktop_lyrics_color_changed(si, ui);
        }
    });

    // 关闭：隐藏 + 回写主窗口开关
    let app_c = app.clone();
    win.on_close_clicked(move || {
        hide();
        if let Some(u) = app_c.upgrade() {
            u.set_desktop_lyrics(false);
            u.invoke_desktop_lyrics_close_requested();
        }
    });

    WIN.with(|s| *s.borrow_mut() = Some(win.as_weak()));
    std::mem::forget(win);
    true
}

/// 底部居中定位（物理坐标，按显示器 DPI 换算）。
fn place_bottom_center(w: &winit::window::Window) {
    let Some(mon) = w.current_monitor().or_else(|| w.primary_monitor()) else {
        return;
    };
    let scale = mon.scale_factor();
    let (origin, size) = (mon.position(), mon.size());
    let win_w = (WIN_W as f64 * scale) as i32;
    let win_h = (WIN_H as f64 * scale) as i32;
    let margin = (110.0 * scale) as i32;
    let x = origin.x + (size.width as i32 - win_w) / 2;
    let y = origin.y + size.height as i32 - win_h - margin;
    w.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
}
