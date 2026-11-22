//! 移动端入口：Android 由 NativeActivity 调用 `android_main`；iOS / 桌面 preview 走 `main`。
//!
//! 共享初始化只做移动端适用部分（存储门面 → 配置 → 缓存）。
//! 桌面专属步骤（托盘、桌面歌词、自启、在线更新、音频设备枚举、全局快捷键等）不在此执行。

use slint::ComponentHandle;

/// 移动端共享初始化。
fn init_common() {
    // 先初始化存储门面（base = 应用数据目录），config 经门面读写
    crate::platform::storage::init(crate::persist::config::data_dir());
    // 日志：core::logger 依赖桌面能力集（simplelog/文件轮转），移动端阶段 1 未纳入，
    // log 宏在无 logger 时是 no-op。
    let cfg = crate::persist::config::AppConfig::load();
    crate::persist::cache::init(&cfg);
    log::info!(
        "[mobile] 数据目录: {} dark={} volume={}",
        crate::persist::config::data_dir().display(),
        cfg.app.dark,
        cfg.app.volume
    );
}

/// 创建并运行移动端窗口（UI 类型由根 crate 的 include_modules 生成）。
fn run_ui() {
    let ui = crate::AppWindowMobile::new().unwrap_or_else(|e| {
        eprintln!("[mobile] 创建 UI 失败: {e}");
        panic!("创建移动端 UI 失败: {e}");
    });
    if let Err(e) = ui.run() {
        eprintln!("[mobile] 事件循环失败: {e}");
        panic!("移动端事件循环失败: {e}");
    }
}

/// Android 入口：由 `android.app.NativeActivity` 通过 lib_name 调起。
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: slint::android::AndroidApp) {
    if let Err(e) = slint::android::init(app) {
        eprintln!("[mobile] slint android init 失败: {e}");
        panic!("slint android init failed: {e}");
    }
    init_common();
    run_ui();
}

/// 非 Android 的移动入口（iOS 宿主 / Windows 桌面 preview）。
#[cfg(all(feature = "mobile", not(target_os = "android")))]
pub fn run_mobile_main() {
    init_common();
    run_ui();
}
