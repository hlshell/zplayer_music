//! 本地视频拖拽播放：通过 Windows API（DragAcceptFiles + SetWindowSubclass）
//! 让主窗口接受从资源管理器拖入的文件，过滤出视频文件后回调播放。
//!
//! 仅限从"本地资源"入口打开的视频播放器启用：
//! - 全局始终注册窗口子类（轻量，仅多一个 WM_DROPFILES 分支）
//! - 通过 ENABLED 原子开关控制是否实际处理拖入文件
//! - 打开本地视频播放器时 set_enabled(true)，关闭时 set_enabled(false)
//!
//! 注意：Slint 1.18.1 的 winit 后端不转发 winit 的 DragDrop 事件，
//! 故直接用 Win32 API 实现。SetWindowSubclass 与 winshell::winmin 的
//! SetWindowLongPtrW 子类可共存（comctl32 子类链在原窗口过程之上）。

#![cfg(target_os = "windows")]

use std::os::raw::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use winapi::shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM};
use winapi::um::commctrl::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use winapi::um::shellapi::{DragFinish, DragQueryFileW};
use winapi::um::winuser::WM_DROPFILES;

const SUBCLASS_ID: usize = 0x5A50_4C52; // "ZPLR" 标识，避免与其他子类冲突

/// 是否启用拖拽处理（仅本地视频播放器打开期间为 true）。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 拖入文件回调：接收视频文件绝对路径列表。
static CALLBACK: Mutex<Option<Box<dyn Fn(Vec<String>) + Send>>> = Mutex::new(None);

/// 启用/禁用拖拽处理。从本地资源打开视频播放器时传 true，关闭时传 false。
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
}

/// 注册拖入文件回调（视频文件路径列表）。在应用启动时调用一次。
pub fn set_callback<F>(cb: F)
where
    F: Fn(Vec<String>) + Send + 'static,
{
    *CALLBACK.lock().unwrap() = Some(Box::new(cb));
}

/// 子类过程：捕获 WM_DROPFILES，提取拖入文件路径，过滤视频后回调。
unsafe extern "system" fn drop_subclass_proc(
    hwnd: winapi::shared::windef::HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
    _uidsubclass: usize,
    _dwrefdata: usize,
) -> LRESULT {
    if msg == WM_DROPFILES && ENABLED.load(Ordering::SeqCst) {
        let hdrop = wparam as winapi::um::shellapi::HDROP;
        if !hdrop.is_null() {
            // 查询拖入文件总数
            let count = unsafe { DragQueryFileW(hdrop, 0xFFFFFFFF, std::ptr::null_mut(), 0) };
            let mut paths: Vec<String> = Vec::with_capacity(count as usize);
            for i in 0..count {
                // 先取路径长度
                let len = unsafe { DragQueryFileW(hdrop, i, std::ptr::null_mut(), 0) };
                if len == 0 {
                    continue;
                }
                let mut buf: Vec<u16> = vec![0u16; (len + 1) as usize];
                let copied = unsafe { DragQueryFileW(hdrop, i, buf.as_mut_ptr(), len + 1) };
                if copied > 0 {
                    if let Some(s) = String::from_utf16(&buf[..copied as usize]).ok() {
                        paths.push(s);
                    }
                }
            }
            unsafe { DragFinish(hdrop) };
            // 过滤视频文件并回调
            let videos: Vec<String> = paths
                .into_iter()
                .filter(|p| is_video_ext(p))
                .collect();
            if !videos.is_empty() {
                if let Some(cb) = CALLBACK.lock().unwrap().as_ref() {
                    cb(videos);
                }
            }
            return 0;
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// 判断路径是否为视频文件扩展名。
fn is_video_ext(path: &str) -> bool {
    let exts = ["mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "m4v", "mpg", "mpeg", "ts", "3gp"];
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    exts.contains(&ext.as_str())
}

/// 安装文件拖拽处理。需在窗口句柄创建后调用（main 启动重试中调用）。
/// 成功返回 true。重复调用安全（已安装则跳过）。
pub fn install(hwnd: *mut c_void) -> bool {
    if hwnd.is_null() {
        return false;
    }
    let hwnd = hwnd as winapi::shared::windef::HWND;
    // 先让窗口接受拖入文件
    unsafe {
        winapi::um::shellapi::DragAcceptFiles(hwnd, 1);
    }
    // SetWindowSubclass 返回非零表示成功
    let ok = unsafe {
        SetWindowSubclass(
            hwnd,
            Some(drop_subclass_proc),
            SUBCLASS_ID,
            0,
        ) != 0
    };
    if ok {
        log::info!(target: "drop", "file drop handler installed");
    } else {
        log::warn!(target: "drop", "SetWindowSubclass failed");
    }
    ok
}

/// 卸载文件拖拽处理（应用退出时调用，通常无需手动调用——窗口销毁时系统自动清理）。
#[allow(dead_code)]
pub fn uninstall(hwnd: *mut c_void) {
    if hwnd.is_null() {
        return;
    }
    let hwnd = hwnd as winapi::shared::windef::HWND;
    unsafe {
        RemoveWindowSubclass(hwnd, Some(drop_subclass_proc), SUBCLASS_ID);
        winapi::um::shellapi::DragAcceptFiles(hwnd, 0);
    }
}
