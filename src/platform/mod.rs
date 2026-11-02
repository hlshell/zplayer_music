//! 平台/系统集成：窗口外壳、开机自启、音频输出（cpal）、存储门面。
pub mod audio;
// 开机自启仅桌面/mobile（web 无此能力）
#[cfg(not(target_arch = "wasm32"))]
pub mod autolaunch;
pub mod storage;
// 窗口外壳（winit_030）仅桌面（web 无原生窗口）
#[cfg(not(target_arch = "wasm32"))]
pub mod winshell;
// 系统托盘仅 Windows 桌面（web/移动端无托盘概念）
#[cfg(windows)]
pub mod tray;
// 文件拖拽播放仅 Windows 桌面（本地视频播放器拖入视频文件）
#[cfg(windows)]
pub mod drop_files;
