//! 窗口外壳相关：DWM 圆角（Windows 11）、最小尺寸钳制（winmin subclass）、
//! 迷你窗口播放列表抽屉开/关（set_mini_drawer，全平台入口，Windows 下联动 winmin）。

use crate::AppWindow;
use slint::winit_030::WinitWindowAccessor;
use slint::ComponentHandle;

// ================= 窗口圆角（Windows 11 DWM 圆角）=================
// 无边框窗口默认是直角；通过 DWMWA_WINDOW_CORNER_PREFERENCE 让 DWM 给窗口加系统圆角。
#[cfg(target_os = "windows")]
mod dwm {
    use slint::winit_030::winit;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle, Win32WindowHandle};

    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: i32 = 2;

    #[link(name = "dwmapi")]
    unsafe extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut core::ffi::c_void,
            attribute: u32,
            value: *const core::ffi::c_void,
            size: u32,
        ) -> i32;
    }

    /// 给窗口设置 DWM 圆角。窗口句柄尚未创建时返回 false（由调用方重试）。
    pub fn apply_rounded_corners(w: &winit::window::Window) -> bool {
        let Ok(handle) = w.window_handle() else { return false; };
        let RawWindowHandle::Win32(Win32WindowHandle { hwnd, .. }) = handle.as_raw() else {
            return false;
        };
        let hwnd = hwnd.get() as *mut core::ffi::c_void;
        let preference = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &preference as *const i32 as *const core::ffi::c_void,
                core::mem::size_of::<i32>() as u32,
            );
        }
        true
    }
}

// ================= 窗口最小尺寸（WINDOWPOSCHANGING 钳制，原生强制）=================
// winit 的 set_min_inner_size 在 Slint Windows 后端不生效（实测可缩到任意小）。
// 这里 subclass 窗口过程，在 WM_WINDOWPOSCHANGING 时把请求的新尺寸钳制到最小尺寸
// （900x560），使窗口不能缩小到最小以下，但可任意放大。
#[cfg(target_os = "windows")]
mod winmin {
    use std::os::raw::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[repr(C)]
    struct WindowPos {
        hwnd: isize,
        hwnd_insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    }

    #[allow(dead_code)] // 预留：WNDPROC 类型，供未来窗口过程 hook
    type WndProc = unsafe extern "system" fn(isize, u32, isize, isize) -> isize;

    static ORIG_PROC: OnceLock<isize> = OnceLock::new();

    const GWLP_WNDPROC: i32 = -4;
    const WM_WINDOWPOSCHANGING: u32 = 0x0046;
    const WM_GETMINMAXINFO: u32 = 0x0024;
    const SWP_NOSIZE: u32 = 0x0001;
    // 最小可缩尺寸（逻辑像素）：900x560，适配小屏幕但仍保留可用布局
    const MIN_W: f64 = 900.0;
    const MIN_H: f64 = 560.0;
    // 迷你窗口模式：处于迷你态时允许缩到迷你尺寸（放行小尺寸钳制）
    pub static MINI: AtomicBool = AtomicBool::new(false);
    // 迷你窗口尺寸（逻辑像素，宽高：395x76 = 72 内容区 + 4 底部进度条；可被拖动微调，仍做下限钳制）
    pub const MINI_W: f64 = 395.0;
    pub const MINI_H: f64 = 76.0;
    // 播放列表抽屉展开时迷你窗口高度：基础 76（含进度条）+ 抽屉区 404（y:76 起，高 404）
    pub const MINI_H_QUEUE: f64 = MINI_H + 404.0;
    // 迷你窗口播放列表抽屉是否展开（展开时窗口增高以容纳向下抽屉）
    pub static MINI_QUEUE: AtomicBool = AtomicBool::new(false);
    // MV 画中画（compact）模式：最小尺寸钳制切换为 480x270（16:9），可放大不可再小
    pub static MV_COMPACT: AtomicBool = AtomicBool::new(false);
    pub const MV_COMPACT_MIN_W: f64 = 480.0;
    pub const MV_COMPACT_MIN_H: f64 = 270.0;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct POINT { pub x: i32, pub y: i32 }

    #[repr(C)]
    struct MinMaxInfo {
        reserved: POINT,
        max_size: POINT,
        max_position: POINT,
        min_track: POINT,
        max_track: POINT,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowLongPtrW(hwnd: isize, index: i32, new_long: isize) -> isize;
        fn CallWindowProcW(prev: isize, hwnd: isize, msg: u32, wparam: isize, lparam: isize) -> isize;
        fn GetDpiForWindow(hwnd: isize) -> u32;
        fn GetWindowRect(hwnd: isize, rect: *mut RECT) -> i32;
        fn MoveWindow(hwnd: isize, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
        fn GetCursorPos(pt: *mut POINT) -> i32;
    }

    /// 读取当前鼠标屏幕坐标（物理像素）。失败返回 false。
    pub fn get_cursor_pos(pt: &mut POINT) -> bool {
        unsafe { GetCursorPos(pt) != 0 }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RECT { left: i32, top: i32, right: i32, bottom: i32 }

    /// 进入/退出迷你窗口模式：切换尺寸钳制目标。
    pub fn set_mini(on: bool) {
        MINI.store(on, Ordering::SeqCst);
    }
    #[allow(dead_code)] // 预留：迷你窗口状态读取，供 UI 查询
    pub fn is_mini() -> bool { MINI.load(Ordering::SeqCst) }

    /// 迷你播放列表抽屉展开状态（影响迷你窗口固定高度）。
    pub fn set_mini_queue(on: bool) {
        MINI_QUEUE.store(on, Ordering::SeqCst);
    }

    /// 进入/退出 MV 画中画模式：最小尺寸钳制目标切换为 480x270。
    pub fn set_mv_compact(on: bool) {
        MV_COMPACT.store(on, Ordering::SeqCst);
    }

    /// 当前迷你窗口高度（逻辑）：抽屉展开时增高以容纳向下抽屉。
    pub fn mini_target_height() -> f64 {
        if MINI_QUEUE.load(Ordering::SeqCst) { MINI_H_QUEUE } else { MINI_H }
    }

    /// 读取窗口当前外框矩形（用于进入迷你前记忆，退出时恢复）。
    pub fn get_outer_rect(hwnd: isize) -> (i32, i32, i32, i32) {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd, &mut r); }
        (r.left, r.top, r.right, r.bottom)
    }

    /// 用 MoveWindow 直接设置窗口位置与尺寸（物理像素，主线程调用）。
    /// 第 6 参 repaint=0：不强制 OS 立即重绘，交给 Slint 在下一帧统一绘制，
    /// 避免切换时 wgpu surface 重建的空帧被 OS 填充为透明/黑（先透明再显示的闪烁）。
    pub fn move_window(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
        unsafe { MoveWindow(hwnd, x, y, w, h, 0); }
    }

    unsafe extern "system" fn min_max_proc(
        hwnd: isize,
        msg: u32,
        wparam: isize,
        lparam: isize,
    ) -> isize {
        // 原生最小跟踪尺寸：系统 resize 模态循环（拖边缘）按拖动侧边缘正确钳制，
        // 边缘数学由 DefWindowProc 处理；迷你模式时下限切换为迷你尺寸。
        // 直接返回 0 不转发：winit 的处理会用 900x560 覆盖迷你模式的小下限。
        // 系统 resize 模态循环（拖边缘）：正常模式 min=900x560；迷你模式 min==max=395x76（固定不可缩放）
        unsafe {
        if msg == WM_GETMINMAXINFO {
            let mmi = lparam as *mut MinMaxInfo;
            if !mmi.is_null() {
                let scale = GetDpiForWindow(hwnd) as f64 / 96.0;
                if MINI.load(Ordering::SeqCst) {
                    // 迷你固定尺寸：min == max，禁止缩放（抽屉展开时高度随之增高）
                    let w = (MINI_W * scale) as i32;
                    let h = (mini_target_height() * scale) as i32;
                    (*mmi).min_track = POINT { x: w, y: h };
                    (*mmi).max_track = POINT { x: w, y: h };
                } else {
                    // MV 画中画：下限 480x270（只设 min，允许放大）；正常：900x560
                    let (min_w, min_h) = if MV_COMPACT.load(Ordering::SeqCst) {
                        (MV_COMPACT_MIN_W, MV_COMPACT_MIN_H)
                    } else {
                        (MIN_W, MIN_H)
                    };
                    (*mmi).min_track = POINT {
                        x: (min_w * scale) as i32,
                        y: (min_h * scale) as i32,
                    };
                }
            }
            return 0;
        }
        // 兜底钳制（覆盖不经系统 resize 循环的 SetWindowPos/MoveWindow 路径）。
        // 正常模式：把请求的新尺寸钳到最小值（900x560）；
        // 迷你模式：强制固定为 MINI 尺寸（拖拽只移动位置，不允许改变大小）。
        if msg == WM_WINDOWPOSCHANGING {
            let wp = lparam as *mut WindowPos;
            if !wp.is_null() && ((*wp).flags & SWP_NOSIZE) == 0 {
                // 关键：SWP_NOSIZE 置位时 cx/cy 是占位值（可能为 0），本次调用不改尺寸；
                // 此时钳制会把无害调用变成真实 resize（最大化刚完成即被拽回最小尺寸的
                // “闪跳”根因），必须跳过。
                let scale = GetDpiForWindow(hwnd) as f64 / 96.0;
                if MINI.load(Ordering::SeqCst) {
                    let mini_cx = (MINI_W * scale) as i32;
                    let mini_cy = (mini_target_height() * scale) as i32;
                    if (*wp).cx != mini_cx || (*wp).cy != mini_cy {
                        (*wp).cx = mini_cx;
                        (*wp).cy = mini_cy;
                        (*wp).flags &= !SWP_NOSIZE;
                    }
                } else {
                    // MV 画中画同样只钳最小尺寸（可放大），复用正常模式的钳制逻辑
                    let (min_w, min_h) = if MV_COMPACT.load(Ordering::SeqCst) {
                        (MV_COMPACT_MIN_W, MV_COMPACT_MIN_H)
                    } else {
                        (MIN_W, MIN_H)
                    };
                    let min_cx = (min_w * scale) as i32;
                    let min_cy = (min_h * scale) as i32;
                    let mut changed = false;
                    if (*wp).cx < min_cx {
                        let (ol, _ot, orp, _ob) = get_outer_rect(hwnd);
                        if (*wp).x != ol {
                            (*wp).x = orp - min_cx;
                        }
                        (*wp).cx = min_cx;
                        changed = true;
                    }
                    if (*wp).cy < min_cy {
                        let (_ol, ot, _orp, ob) = get_outer_rect(hwnd);
                        if (*wp).y != ot {
                            (*wp).y = ob - min_cy;
                        }
                        (*wp).cy = min_cy;
                        changed = true;
                    }
                    if changed {
                        (*wp).flags &= !SWP_NOSIZE;
                    }
                }
            }
        }
        CallWindowProcW(*ORIG_PROC.get().unwrap_or(&0), hwnd, msg, wparam, lparam)
        }
    }

    /// 安装最小尺寸限制（替换窗口过程，链式转发）。成功返回 true。
    pub fn install(hwnd: *mut c_void) -> bool {
        if ORIG_PROC.get().is_some() {
            return true;
        }
        let prev = unsafe { SetWindowLongPtrW(hwnd as isize, GWLP_WNDPROC, min_max_proc as *const () as isize) };
        if prev == 0 {
            return false;
        }
        let _ = ORIG_PROC.set(prev);
        true
    }
}

/// 迷你窗口播放列表抽屉开/关：同步 Slint 状态 + winmin 抽屉标志 + 调整窗口高度
/// （关=80，开=增高到 MINI_H_QUEUE 容纳向下抽屉）。
pub(crate) fn set_mini_drawer(u: &AppWindow, open: bool) {
    u.set_mini_queue_open(open);
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        winmin::set_mini_queue(open);
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
            let r = winmin::get_outer_rect(hwnd);
            let h = (winmin::mini_target_height() * scale) as i32;
            winmin::move_window(hwnd, r.0, r.1, (winmin::MINI_W * scale) as i32, h);
        }
    }
}

/// 迷你窗口模式相关（供 main 的迷你窗口逻辑直接调用）。
#[cfg(target_os = "windows")]
pub(crate) use winmin::{
    get_cursor_pos, get_outer_rect, move_window, set_mini, set_mini_queue, install, MINI_H,
    MINI_W, POINT,
};
/// MV 画中画模式（供 handlers/mv/window.rs 调用）。
#[cfg(target_os = "windows")]
pub(crate) use winmin::{set_mv_compact, MV_COMPACT_MIN_H, MV_COMPACT_MIN_W};
/// Windows DWM 圆角（供 main 启动重试调用）。
#[cfg(target_os = "windows")]
pub(crate) use dwm::apply_rounded_corners;
