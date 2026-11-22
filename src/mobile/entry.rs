//! 移动端入口：Android 由 NativeActivity 调用 `android_main`；iOS / 桌面 preview 走 `main`。
//!
//! 共享初始化只做移动端适用部分（存储门面 → 配置 → 缓存）。
//! 桌面专属步骤（托盘、桌面歌词、自启、在线更新、音频设备枚举、全局快捷键等）不在此执行。
//!
//! 本文件只由 lib 编译（bin 在 mobile feature 下经 `crate::mobile::entry` 调用），
//! 业务模块直接用 `crate::` 路径。

use slint::ComponentHandle;
use std::sync::{Arc, OnceLock};

/// 首页数据加载用的 tokio 运行时：进程内常驻。
/// 若随初始化函数返回而 drop，运行时会被关闭，进行中的加载/封面任务全部取消。
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// 移动端共享初始化；返回已加载的配置，供 UI 与网络初始化复用（避免二次读盘）。
fn init_common() -> crate::persist::config::AppConfig {
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
    cfg
}

/// 首页数据加载：与桌面 main 同一条链路（NeteaseClient → tokio 运行时 →
/// load::load_initial），仅省略托盘/桌面歌词/自启/在线更新/音频设备枚举等桌面专属步骤。
/// 在 UI 创建之后调用，后台任务填充 UI 属性，不阻塞事件循环；失败只记录不 panic。
fn start_home_load(ui: &crate::AppWindow, cfg: &crate::persist::config::AppConfig) {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[mobile] tokio 运行时创建失败: {e}");
            return;
        }
    };
    let handle = rt.handle().clone();
    // 运行时放进静态 OnceLock 常驻；重复调用（不应发生）时直接跳过
    if RUNTIME.set(rt).is_err() {
        eprintln!("[mobile] tokio 运行时已初始化，跳过首页加载");
        return;
    }
    // 网络配置（代理 / realip）：各 reqwest client 构建时读取
    crate::network::proxy::init(crate::network::proxy::NetOptions {
        proxy_enabled: cfg.network.proxy_enabled,
        proxy_type_index: cfg.network.proxy_type_index,
        proxy_host: cfg.network.proxy_host.clone(),
        proxy_port: cfg.network.proxy_port,
        realip_enabled: cfg.network.realip_enabled,
        realip_addr: cfg.network.realip_addr.clone(),
    });
    // 封面门面（cover_warm / cover_now）需要运行时句柄在 UI 线程发起后台任务
    crate::load::set_runtime(handle.clone());
    let client = Arc::new(crate::network::netease::client::NeteaseClient::new());
    // load_initial 只读写移动 AppWindow 已补齐的同名属性，桌面/移动共用同一实现
    handle.spawn(crate::load::home::load_initial(
        client,
        ui.as_weak(),
        cfg.clone(),
    ));
}

/// 创建并运行移动端窗口（UI 类型由 lib 的 include_modules 生成；
/// 移动构建下 `AppWindow` 即 ui/mobile/appwindow-mobile.slint 的窗口组件）。
fn run_ui(cfg: crate::persist::config::AppConfig) {
    let ui = crate::AppWindow::new().unwrap_or_else(|e| {
        eprintln!("[mobile] 创建 UI 失败: {e}");
        panic!("创建移动端 UI 失败: {e}");
    });
    // 数据加载放到 UI 创建之后（需要 as_weak），后台执行
    start_home_load(&ui, &cfg);
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
    let cfg = init_common();
    run_ui(cfg);
}

/// 非 Android 的移动入口（iOS 宿主 / Windows 桌面 preview）。
#[cfg(all(feature = "mobile", not(target_os = "android")))]
pub fn run_mobile_main() {
    let cfg = init_common();
    run_ui(cfg);
}
