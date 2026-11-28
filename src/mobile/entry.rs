//! 移动端入口：Android 由 NativeActivity 调用 `android_main`；iOS / 桌面 preview 走 `main`。
//!
//! 共享初始化只做移动端适用部分（存储门面 → 配置 → 缓存）。
//! 桌面专属步骤（托盘、桌面歌词、自启、在线更新、音频设备枚举、全局快捷键等）不在此执行。
//!
//! 本文件只由 lib 编译（bin 在 mobile feature 下经 `crate::mobile::entry` 调用），
//! 业务模块直接用 `crate::` 路径。

use slint::ComponentHandle;
use std::sync::{Arc, OnceLock};

/// NDK 27 的 libc++（静态与共享库）均已移除 `std::__ndk1::__hash_memory`，而 Slint
/// 预编译 Skia 静态库（旧版 NDK 构建）仍引用它，运行时 dlopen 报
/// `cannot locate symbol "_ZNSt6__ndk113__hash_memoryEPKvm"`。
/// 用 export_name 补出该 C++ mangled 符号：FNV-1a 稳定哈希。Skia 内部的
/// unordered_map 只要求进程内确定性，不要求与 libc++ 原算法一致。
#[cfg(target_os = "android")]
#[unsafe(export_name = "_ZNSt6__ndk113__hash_memoryEPKvm")]
pub extern "C" fn skia_hash_memory_shim(ptr: *const u8, len: usize) -> usize {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    if !ptr.is_null() {
        for i in 0..len {
            // SAFETY: 调用方（Skia/libc++ 约定）保证 ptr 指向 len 字节的连续有效内存。
            h ^= unsafe { *ptr.add(i) } as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h as usize
}

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
    // 收藏列表跨启动恢复（播放层爱心状态由 handlers 按当前歌曲同步）
    crate::persist::liked::load_liked();
    // 最近播放记录跨启动恢复
    crate::persist::recent::load_recent();
    // 搜索历史跨启动恢复（搜索页无关键词时展示）
    crate::persist::history::load_history();
    // 播放队列快照跨启动恢复（含当前曲目与进度）
    crate::persist::queue::init();
    // 坏歌标记跨启动恢复（列表标示与自动跳过）
    crate::persist::bad::load();
    log::info!(
        "[mobile] 数据目录: {} dark={} volume={}",
        crate::persist::config::data_dir().display(),
        cfg.app.dark,
        cfg.app.volume
    );
    cfg
}

/// 创建进程内常驻的 tokio 运行时并返回句柄。
/// 运行时绑定进程生命周期（`OnceLock`）；**重复调用必须返回同一个句柄**，
/// 否则 Activity 重建 / 再次进入 `android_main` 时会拿不到运行时，
/// 首页数据不会重新加载（表现为切回应用后所有列表为空）。
fn init_runtime() -> tokio::runtime::Handle {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Runtime::new().expect("[mobile] tokio 运行时创建失败")
        })
        .handle()
        .clone()
}

/// 首页数据加载 + 播放接线：与桌面 main 同一条链路（NeteaseClient → tokio 运行时 →
/// load::load_initial），仅省略托盘/桌面歌词/自启/在线更新/音频设备枚举等桌面专属步骤。
/// 在 UI 创建之后调用，后台任务填充 UI 属性，不阻塞事件循环；失败只记录不 panic。
fn start_home_load(
    ui: &crate::AppWindow,
    cfg: crate::persist::config::AppConfig,
    handle: tokio::runtime::Handle,
) {
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
    // 播放/首页回调先于数据加载接线，保证首页数据到达后点歌立即可用
    crate::mobile::handlers::setup(ui, handle.clone(), client.clone());
    // load_initial 只读写移动 AppWindow 已补齐的同名属性，桌面/移动共用同一实现
    handle.spawn(crate::load::home::load_initial(
        client,
        ui.as_weak(),
        cfg,
    ));
}

/// Android：保存 Activity 的 JNI 指针（用于回调自定义 MainActivity）。
///
/// 黑帧来自 NativeActivity 的 SurfaceView（一块不透明的系统黑 surface），与 Slint
/// 渲染器无关，无法在纯渲染层消除。最终方案改为 Gradle + 自定义 `MainActivity`（Java）：
/// 用系统启动屏保持到 Slint 首帧、并用覆盖 View 挡住切回重建。Rust 侧只需两处 JNI 回调：
///   - 首帧已渲染 → `onFirstFrameRendered()` 撤除系统启动屏；
///   - 配置恢复后 → `setThemeBackground(argb)` 同步窗口/遮盖底色。
#[cfg(target_os = "android")]
static ACTIVITY_PTR: std::sync::atomic::AtomicPtr<std::ffi::c_void> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// 在 Activity 上调用指定 Java 方法；失败只记录，不影响主流程。
#[cfg(target_os = "android")]
fn android_call_activity(method: &str, sig: &str, args: &[jni::objects::JValue]) {
    use std::sync::atomic::Ordering;
    let ptr = ACTIVITY_PTR.load(Ordering::Acquire);
    if ptr.is_null() {
        log::warn!("[boot] Activity 指针为空，跳过 JNI 调用 {method}");
        return;
    }
    fn inner(
        ptr: *mut std::ffi::c_void,
        method: &str,
        sig: &str,
        args: &[jni::objects::JValue],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use jni::objects::JObject;
        let ctx = ndk_context::android_context();
        let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        let act = unsafe { JObject::from_raw(ptr.cast()) };
        let r = env.call_method(&act, method, sig, args);
        // 必须清掉挂起异常，否则 JNI 后续调用会 abort 整个进程
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return Err("Java 调用抛异常".into());
        }
        r?;
        Ok(())
    }
    if let Err(e) = inner(ptr, method, sig, args) {
        log::warn!("[boot] JNI 调用 {method} 失败: {e}");
    }
}

/// Slint 首帧真正上屏后调用：通知 Java 撤除系统启动屏。
#[cfg(target_os = "android")]
fn android_notify_first_frame() {
    android_call_activity("onFirstFrameRendered", "()V", &[]);
}

/// 配置恢复后调用：把窗口/切回遮盖底色同步为主题底色。
#[cfg(target_os = "android")]
fn android_set_window_background(argb: u32) {
    use jni::objects::JValue;
    android_call_activity("setThemeBackground", "(I)V", &[JValue::Int(argb as i32)]);
}

/// 用系统浏览器打开链接（Android 走 JNI Intent；桌面预览走系统命令）。
/// 用于「检查更新 → 前往下载」与 MV 分类条目（移动端无视频解码栈）。
pub fn open_url(url: &str) {
    #[cfg(target_os = "android")]
    android_open_url(url);
    #[cfg(all(not(target_os = "android"), not(target_os = "ios")))]
    {
        let bin = if cfg!(windows) {
            "cmd"
        } else if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let mut cmd = std::process::Command::new(bin);
        if cfg!(windows) {
            cmd.args(["/C", "start", "", url]);
        } else {
            cmd.arg(url);
        }
        if let Err(e) = cmd.spawn() {
            log::warn!("[ui] 打开链接失败: {e}");
        }
    }
    #[cfg(target_os = "ios")]
    {
        log::warn!("[ui] iOS 尚未实现打开链接: {url}");
    }
}

/// Android：调用 MainActivity.openUrl(String)，由 Java 侧以 ACTION_VIEW 拉起浏览器。
#[cfg(target_os = "android")]
fn android_open_url(url: &str) {
    use jni::objects::{JObject, JValue};
    let ptr = ACTIVITY_PTR.load(std::sync::atomic::Ordering::Acquire);
    if ptr.is_null() {
        log::warn!("[ui] Activity 指针为空，无法打开链接");
        return;
    }
    let ctx = ndk_context::android_context();
    let vm = match unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) } {
        Ok(vm) => vm,
        Err(e) => {
            log::warn!("[ui] JNI JavaVM 获取失败: {e}");
            return;
        }
    };
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            log::warn!("[ui] JNI attach 失败: {e}");
            return;
        }
    };
    let jurl = match env.new_string(url) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("[ui] 创建 jstring 失败: {e}");
            return;
        }
    };
    let jurl_obj = JObject::from(jurl);
    let act = unsafe { JObject::from_raw(ptr.cast()) };
    let r = env.call_method(
        &act,
        "openUrl",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&jurl_obj)],
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        log::warn!("[ui] openUrl 抛异常");
        return;
    }
    if let Err(e) = r {
        log::warn!("[ui] openUrl 调用失败: {e}");
    }
}

/// 创建窗口组件（UI 类型由 lib 的 include_modules 生成）。
fn new_ui() -> crate::AppWindow {
    crate::AppWindow::new().unwrap_or_else(|e| {
        eprintln!("[mobile] 创建 UI 失败: {e}");
        panic!("创建移动端 UI 失败: {e}");
    })
}

/// 桌面预览截图自检（MOBILE_SHOT=1 时启用）：强制深色 → 逐页抓 home/search/recent/
/// settings/player，再补一张浅色 home，保存到工作区根目录后退出。仅用于玻璃对齐与
/// 功能验收的视觉回归；正常运行不设该环境变量时完全不介入。
#[cfg(all(not(target_os = "android"), not(target_os = "ios")))]
fn arm_screenshots(ui: &crate::AppWindow) {
    use slint::ComponentHandle;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::Duration;

    if std::env::var_os("MOBILE_SHOT").is_none() {
        return;
    }
    let dir = std::env::var("MOBILE_SHOT_DIR").unwrap_or_else(|_| ".".to_string());
    let snap = move |ui_weak: &slint::Weak<crate::AppWindow>, name: &str| {
        let Some(u) = ui_weak.upgrade() else {
            eprintln!("[shot] {name}: ui gone");
            return;
        };
        let Ok(mut s) = u.window().take_snapshot() else {
            eprintln!("[shot] {name}: take_snapshot failed");
            return;
        };
        let (w, h) = (s.width(), s.height());
        let data: Vec<u8> = s.make_mut_bytes().to_vec();
        let path = format!("{dir}/{name}");
        match image::RgbaImage::from_raw(w, h, data) {
            Some(img) => match img.save(&path) {
                Ok(_) => eprintln!("[shot] saved {path} ({w}x{h})"),
                Err(e) => eprintln!("[shot] {name}: save failed: {e}"),
            },
            None => eprintln!("[shot] {name}: from_raw failed ({w}x{h})"),
        }
    };

    // 起手强制深色，保证文件名与主题一致；首页深色 → 浅色 → 回到深色逐页抓
    ui.set_dark(true);
    let step = Rc::new(Cell::new(0u32));
    let timer = Rc::new(slint::Timer::default());
    let t = timer.clone();
    let s = step.clone();
    let uw = ui.as_weak();
    // 1000ms 一拍：前 6 拍等 splash(1.25s) + 网络数据，之后每页留 1s 渲染/切换
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(1000), move || {
        let cur = s.get();
        match cur {
            0..=5 => {} // 等待 splash + 首页数据
            6 => {
                snap(&uw, "_shot_home.png");
                if let Some(u) = uw.upgrade() {
                    u.set_dark(false);
                }
            }
            7 => {
                snap(&uw, "_shot_home_light.png");
                if let Some(u) = uw.upgrade() {
                    u.set_dark(true);
                    u.set_current_nav(1); // 搜索
                }
            }
            8 => {
                snap(&uw, "_shot_search.png");
                if let Some(u) = uw.upgrade() {
                    let _ = u.invoke_search_submitted("晚风".into(), 0); // 触发真实搜索
                }
            }
            9 => {
                snap(&uw, "_shot_search_loading.png");
            }
            10 => {
                snap(&uw, "_shot_search_result.png");
                // 回到未搜索态（清空关键词与结果）→ 下一拍抓搜索历史
                if let Some(u) = uw.upgrade() {
                    let _ = u.invoke_search_submitted("".into(), 0);
                }
            }
            11 => {
                snap(&uw, "_shot_search_history.png");
                if let Some(u) = uw.upgrade() {
                    u.set_current_nav(2); // 最近
                }
            }
            12 => {
                snap(&uw, "_shot_recent.png");
                if let Some(u) = uw.upgrade() {
                    u.set_current_nav(3); // 设置
                }
            }
            13 => {
                snap(&uw, "_shot_settings.png");
                if let Some(u) = uw.upgrade() {
                    u.set_current_nav(0); // 回首页
                    let _ = u.invoke_hero_opened(); // 打开每日推荐详情
                }
            }
            14 => {
                snap(&uw, "_shot_detail.png");
            }
            15 => {
                snap(&uw, "_shot_detail_loaded.png");
                if let Some(u) = uw.upgrade() {
                    let _ = u.invoke_detail_back();
                    u.set_player_open(true);
                }
            }
            16 => {
                snap(&uw, "_shot_player.png");
                // 强制进入加载态，抓取"正在加载音频…"反馈
                if let Some(u) = uw.upgrade() {
                    u.set_player_loading(true);
                }
            }
            17 => {
                snap(&uw, "_shot_player_loading.png");
                // 横屏：验证 NavRail + 玻璃无回归
                if let Some(u) = uw.upgrade() {
                    u.set_player_loading(false);
                    u.set_player_open(false);
                    let _ = u.window().set_size(slint::LogicalSize::new(844.0, 390.0));
                    u.set_current_nav(0);
                }
            }
            18 => {
                snap(&uw, "_shot_landscape.png");
                // 横屏打开播放页：验证分栏下左侧不再重复显示局部歌词
                if let Some(u) = uw.upgrade() {
                    u.set_player_open(true);
                }
            }
            19 => {
                snap(&uw, "_shot_landscape_player.png");
                // 平板：验证 NavSidebar + 双栏/玻璃无回归
                if let Some(u) = uw.upgrade() {
                    u.set_player_open(false);
                    let _ = u.window().set_size(slint::LogicalSize::new(1100.0, 850.0));
                }
            }
            20 => {
                snap(&uw, "_shot_tablet.png");
            }
            21 => {
                // 竖屏进入分类浏览（歌单）：复现底栏/迷你条遮挡
                if let Some(u) = uw.upgrade() {
                    let _ = u.window().set_size(slint::LogicalSize::new(390.0, 844.0));
                    u.invoke_browse_opened(0);
                }
            }
            22 => {
                snap(&uw, "_shot_browse_portrait.png");
                // 横屏分类浏览：复现 NavRail 与内容重叠
                if let Some(u) = uw.upgrade() {
                    let _ = u.window().set_size(slint::LogicalSize::new(844.0, 390.0));
                }
            }
            23 => {
                snap(&uw, "_shot_browse_landscape.png");
            }
            24 => {
                // 电台骨架屏：不触发网络，直接置加载态复现背景色带
                if let Some(u) = uw.upgrade() {
                    let _ = u.window().set_size(slint::LogicalSize::new(390.0, 844.0));
                    u.set_browse_kind(3);
                    u.set_browse_title("电台".into());
                    u.set_browse_items(Default::default());
                    u.set_browse_loading(true);
                    u.set_current_nav(4);
                }
            }
            25 => {
                snap(&uw, "_shot_radio_skeleton.png");
            }
            26 => {
                // 歌曲操作抽屉：取首页热门第一名打开（模拟长按 / ⋯）
                if let Some(u) = uw.upgrade() {
                    let _ = u.window().set_size(slint::LogicalSize::new(390.0, 844.0));
                    u.set_current_nav(0);
                    let m = u.get_hot_songs();
                    if let Some(h) = slint::Model::row_data(&m, 0) {
                        u.invoke_song_more(h.id, 0);
                    }
                }
            }
            27 => {
                snap(&uw, "_shot_song_sheet.png");
            }
            _ => {
                t.stop();
                let _ = slint::quit_event_loop();
            }
        }
        s.set(cur + 1);
    });
    std::mem::forget(timer);
    eprintln!("[shot] armed");
}

/// 创建并运行移动端窗口（iOS / 桌面 preview 路径；Android 走首帧后初始化）。
fn run_ui(cfg: crate::persist::config::AppConfig) {
    let ui = new_ui();
    // 平台注入：0 桌面预览 / 1 Android（Material）/ 2 iOS（玻璃）
    let platform = if cfg!(target_os = "android") {
        1
    } else if cfg!(target_os = "ios") {
        2
    } else {
        0
    };
    ui.global::<crate::MobileTheme>().set_platform(platform);
    // 运行时在 UI 创建后初始化；数据加载与回调接线共用同一个运行时
    start_home_load(&ui, cfg, init_runtime());
    // 截图自检（仅 MOBILE_SHOT=1 时介入，桌面预览专用）
    #[cfg(all(not(target_os = "android"), not(target_os = "ios")))]
    arm_screenshots(&ui);
    log::info!("[boot] ui-created, entering event loop");
    if let Err(e) = ui.run() {
        eprintln!("[mobile] 事件循环失败: {e}");
        panic!("移动端事件循环失败: {e}");
    }
}
/// Android 入口：由 `android.app.NativeActivity` 通过 lib_name 调起。
///
/// 启动时序优化（消除"原生 splash → 黑屏 → Slint 启动动画"的断层）：
/// 先只做 `.so` 内最轻的初始化并立刻创建 UI / 进入事件循环，让 Slint 首帧
/// （启动页锚帧：纯色 + 完整 logo）尽快上屏；配置恢复、缓存加载、tokio 运行时、
/// 网络客户端等全部推迟到首帧之后的第一个事件循环回调里执行。
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(app: slint::android::AndroidApp) {
    // 注入应用私有目录：Android 上 dirs::data_dir() 不可用，配置/收藏需落到沙盒内才能持久化
    if let Some(dir) = app.internal_data_path() {
        crate::persist::config::set_data_root(dir);
    }
    // activity 的 JNI 指针必须在 init 消耗 app 之前取出（用于后续 JNI 回调）
    let activity_ptr = app.activity_as_ptr();
    ACTIVITY_PTR.store(activity_ptr, std::sync::atomic::Ordering::Release);
    if let Err(e) = slint::android::init(app) {
        eprintln!("[mobile] slint android init 失败: {e}");
        panic!("slint android init failed: {e}");
    }
    // 初始化 logcat 日志：此前移动端无 logger，log::info/warn 全是 no-op，无法排查数据加载。
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("zplayer"),
    );
    log::info!("[boot] slint-init done");

    // 首帧最小路径：只创建 UI 并排队重初始化，随即进入事件循环出首帧
    let ui = new_ui();
    ui.global::<crate::MobileTheme>().set_platform(1);

    // 渲染回调驱动撤遮盖：每次 AfterRendering（真正的帧已绘制）节流 200ms 通知 Java。
    // Java 侧 onFirstFrameRendered 只在遮盖层可见时撤除，因此：
    //   - 冷启动：首个 AfterRendering 到达 → 撤遮盖；
    //   - 切回前台 surface 重建：surfaceCreated 重新显示遮盖 → 其后首个 AfterRendering 撤除。
    // 不再依赖固定延时猜测，彻底避免"遮盖撤早了露黑"或"撤晚了卡住"。
    static LAST_NOTIFY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    {
        let result = ui.window().set_rendering_notifier(move |state, _gfx| {
            use std::sync::atomic::Ordering;
            if let slint::RenderingState::AfterRendering = state {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let last = LAST_NOTIFY_MS.load(Ordering::Relaxed);
                if now.saturating_sub(last) >= 200 {
                    LAST_NOTIFY_MS.store(now, Ordering::Relaxed);
                    android_notify_first_frame();
                }
            }
        });
        if let Err(e) = result {
            log::warn!("[boot] 注册首帧渲染通知失败: {e:?}");
        }
    }

    let ui_weak = ui.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        // 以下为原 run_ui 的重活：首帧（启动页）已上屏后再执行
        let cfg = init_common();
        log::info!("[boot] config+data restored");
        // 首帧已上屏：窗口背景从"启动页"切到当前主题底色。
        // surface 始终保持透明，此后从后台切回时重建的 surface 透出的是主题底色，
        // 不会闪黑、也不会闪启动页 logo。
        android_set_window_background(if cfg.app.dark { 0xFF12_1212 } else { 0xFFF3_F1FB });
        if let Some(ui) = ui_weak.upgrade() {
            start_home_load(&ui, cfg, init_runtime());
        }
    });

    log::info!("[boot] ui-created, entering event loop");
    if let Err(e) = ui.run() {
        eprintln!("[mobile] 事件循环失败: {e}");
        panic!("移动端事件循环失败: {e}");
    }
}

/// 非 Android 的移动入口（iOS 宿主 / Windows 桌面 preview）。
#[cfg(all(feature = "mobile", not(target_os = "android")))]
pub fn run_mobile_main() {
    let cfg = init_common();
    run_ui(cfg);
}
