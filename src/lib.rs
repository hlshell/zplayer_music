//! 共享库根：供 bin（桌面）和移动端 cdylib/staticlib 复用业务模块。
//!
//! lib 也调用 `slint::include_modules!()`：lib 同样编译 handlers 等模块，它们直接引用
//! `crate::AppWindow` 等生成类型，不 include 则 lib 无法独立编译。UI 文件由 build.rs 按
//! 目标平台选定，lib 与 bin 用的是同一份文件（各自 crate 内生成独立符号，不产生冲突）；
//! 要避免的是 lib 与 bin include 不同 UI。
//!
//! 门控与 bin 保持一致，覆盖 desktop / web / wasm32：web(wasm32) 构建命令为
//! `--no-default-features --features web`，此时 desktop feature 不开启，必须显式覆盖。

// lib 与 bin 各编译一份模块，bin 不使用 lib 中的这些项（尚未迁移），
// 桌面/web 构建下 lib 会报大量 dead_code；移动端由 mobile 入口真实使用，不屏蔽。
#![cfg_attr(not(feature = "mobile"), allow(dead_code))]

#[cfg(any(feature = "desktop", feature = "web", target_arch = "wasm32"))]
slint::include_modules!();

#[cfg(not(target_arch = "wasm32"))]
mod core;
#[cfg(not(target_arch = "wasm32"))]
pub use core::{fetch, logger, lyrics, queue, song_util};

mod platform;
pub use platform::audio as player;

#[cfg(not(target_arch = "wasm32"))]
mod persist;
#[cfg(not(target_arch = "wasm32"))]
pub use persist::{cache, config, liked, recent, state};
#[cfg(not(target_arch = "wasm32"))]
mod network;
#[cfg(not(target_arch = "wasm32"))]
pub use network::{netease, service, sources};
#[cfg(not(target_arch = "wasm32"))]
mod load;
#[cfg(not(target_arch = "wasm32"))]
mod handlers;
#[cfg(not(target_arch = "wasm32"))]
mod i18n;

// 启动打点基准；跨模块统一打点用 crate::boot_elapsed_ms()（与 bin 入口同一语义）
static BOOT_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
pub fn boot_elapsed_ms() -> u128 {
    BOOT_T0.get_or_init(std::time::Instant::now).elapsed().as_millis()
}

#[cfg(feature = "mobile")]
pub mod mobile;

pub mod ui;
