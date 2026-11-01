// Windows 下按 GUI 子系统编译（不弹额外命令窗口）；Linux/macOS 不受影响
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
// wasm 构建无 main：由 cfg(wasm32) 的 web_main 提供 wasm_bindgen(start) 入口
#![cfg_attr(target_arch = "wasm32", no_main)]

slint::include_modules!();

// ---- 从 src-tauri 迁移的后端纯 Rust 核心 ----
// web(wasm) 最小闭环自包含（不编译 core：其 fetch/logger 依赖 reqwest/simplelog，
// 由 web_main 内嵌 gdmusic 解析 + console_log 替代，阶段 1b 再做 core 的 web 移植）
#[cfg(not(target_arch = "wasm32"))]
mod core;
#[cfg(not(target_arch = "wasm32"))]
pub use core::{fetch, logger, lyrics, queue, song_util};
// wasm 仅复用 core::lyrics（LyricLine 模型构造，纯 std+slint 类型无平台依赖；
// fetch/logger 依赖 reqwest/simplelog 仍不进 web 构建）
#[cfg(target_arch = "wasm32")]
mod core {
    pub mod lyrics;
}
#[cfg(target_arch = "wasm32")]
pub use core::lyrics;
mod platform;
#[cfg(not(target_arch = "wasm32"))]
pub use platform::{autolaunch, winshell};
pub use platform::audio as player;

// ---- 桌面/mobile 专属模块（web 最小闭环不编译；web 入口见下方 cfg(wasm) 段）----
#[cfg(not(target_arch = "wasm32"))]
mod persist;
#[cfg(not(target_arch = "wasm32"))]
pub use persist::{cache, config, liked, recent, state};
#[cfg(not(target_arch = "wasm32"))]
mod network;
#[cfg(not(target_arch = "wasm32"))]
pub use network::{netease, service, sources};
#[cfg(not(target_arch = "wasm32"))]
pub use network::proxy as net;
#[cfg(not(target_arch = "wasm32"))]
mod load;
#[cfg(not(target_arch = "wasm32"))]
mod handlers;
#[cfg(not(target_arch = "wasm32"))]
mod i18n;
// web(wasm32) 业务层：gdmusic 数据通道 + 按业务拆分的回调接线（web_main 编排 setup_all）
#[cfg(target_arch = "wasm32")]
mod web;

use slint::ComponentHandle;
#[cfg(not(target_arch = "wasm32"))]
use slint::winit_030::winit;
#[cfg(not(target_arch = "wasm32"))]
use slint::winit_030::WinitWindowAccessor;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::network::netease::client::NeteaseClient;

// 运行时窗口图标（任务栏/标题栏/Alt+Tab）：无边框窗口默认任务栏图标空白，
// 解码编译期嵌入的 logo 为 RGBA 设置 winit 窗口图标。
// 必须在事件循环启动后调用 —— 事件循环未跑时 with_winit_window 静默失败
// （回调永不执行），曾导致任务栏/Alt+Tab 图标空白。仅桌面（web 无 winit）。
#[cfg(not(target_arch = "wasm32"))]
fn set_runtime_icon(ui: &AppWindow) {
    use image::GenericImageView;
    let Ok(dyn_img) = image::load_from_memory(include_bytes!("../ui/assets/app-icon.png")) else { return };
    let (w, h) = dyn_img.dimensions();
    let rgba = dyn_img.to_rgba8().into_raw();
    let _ = ui.window().with_winit_window(move |win: &slint::winit_030::winit::window::Window| {
        if let Ok(icon) = slint::winit_030::winit::window::Icon::from_rgba(rgba, w, h) {
            win.set_window_icon(Some(icon));
        }
    });
}


// 启动打点基准（main 入口时刻）；跨模块统一打点用 crate::boot_elapsed_ms()（同一时间基准）
static BOOT_T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
pub fn boot_elapsed_ms() -> u128 {
    BOOT_T0.get_or_init(std::time::Instant::now).elapsed().as_millis()
}

// ---- 桌面/mobile 入口（web 最小闭环不编译，见下方 cfg(wasm) 段）----
#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), slint::PlatformError> {
    // 启动性能打点：各阶段累计耗时写日志，定位启动慢的阶段（低频输出，保留无碍）
    let t0 = *BOOT_T0.get_or_init(std::time::Instant::now);
    macro_rules! boot_mark {
        ($name:expr) => {
            log::info!("[boot] {} {}ms", $name, t0.elapsed().as_millis())
        };
    }
    // ---- 日志 + 配置 + 缓存初始化（统一跨平台数据目录）----
    // 先初始化存储门面（base = 应用数据目录），config 经门面读写
    crate::platform::storage::init(crate::persist::config::data_dir());
    logger::init();
    let cfg = crate::persist::config::AppConfig::load();
    crate::persist::cache::init(&cfg);
    // 旧版平铺封面迁移到 legacy/（分类子目录重构后兼容命中）
    crate::persist::cache::migrate_legacy_covers();
    liked::load_liked();
    recent::load_recent();
    crate::persist::history::load_history();
    log::info!("[config] 数据目录: {}", crate::persist::config::data_dir().display());
    log::info!("[config] dark={} volume={}", cfg.app.dark, cfg.app.volume);

    // ---- 无原生边框窗口（自定义标题栏）+ 记忆上次窗口位置 ----
    // 1) 通过 Slint 官方 winit 钩子在创建窗口时请求关闭系统装饰；
    // 2) appwindow.slint 中已设 no-frame: true（无边框）+ resize-border-width（边缘缩放）。
    // 3) 若上次退出时保存了位置，则在创建窗口时直接定位（避免闪烁）。
    let saved_pos = cfg.app.window_x.zip(cfg.app.window_y);
    // 优先 OpenGL 硬件渲染（femtovg）：无 graphics-api 要求时 Slint 的驱动黑名单
    // 会把部分 GPU 静默回退到软件渲染，全窗口 60fps 软件光栅化实测空闲 CPU 15%+；
    // 强制 GL 后空闲 CPU 降至 0.3%。GL 不可用的机器回落默认选择（软件渲染兜底）。
    let hook = move |attrs: winit::window::WindowAttributes| {
        let mut attrs = attrs.with_decorations(false);
        // 创建时先隐藏窗口：winit 窗口创建即显示，首帧渲染前是透明空窗（透出桌面壁纸）；
        // 事件循环启动、首帧就绪后再 ui.show()（见 pre-run 前），消除启动透明空窗
        attrs = attrs.with_visible(false);
        if let Some((x, y)) = saved_pos {
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        // 窗口最小尺寸 = 900x560：创建时即限制（适配小屏幕，避免缩小挤压）
        attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(900.0, 560.0));
        attrs
    };
    // UI 自动化测试模式（ZPLAYER_UI_TEST=1）：强制软件渲染。OpenGL/wgpu 窗口像素经 GPU
    // 硬件层合成，GDI 截图（CopyFromScreen/PrintWindow）无法捕获，软渲染才可被外部截图验证。
    if std::env::var_os("ZPLAYER_UI_TEST").is_some() {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .with_winit_window_attributes_hook(hook.clone())
            .select()?;
    } else {
        let gl_sel = slint::BackendSelector::new()
            .require_opengl()
            .with_winit_window_attributes_hook(hook.clone());
        if gl_sel.select().is_err() {
            log::warn!("[backend] OpenGL 初始化失败，回退默认渲染后端（软件渲染）");
            let hook_pos = saved_pos;
            slint::BackendSelector::new()
                .with_winit_window_attributes_hook(move |attrs: winit::window::WindowAttributes| {
                    let mut attrs = attrs.with_decorations(false);
                    // 同 GL 分支：首帧渲染就绪前隐藏窗口，避免透明空窗
                    attrs = attrs.with_visible(false);
                    if let Some((x, y)) = hook_pos {
                        attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(x, y));
                    }
                    attrs = attrs.with_min_inner_size(winit::dpi::LogicalSize::new(900.0, 560.0));
                    attrs
                })
                .select()?;
        }
    }

    boot_mark!("gl-select+backend");
    let ui = AppWindow::new()?;
    boot_mark!("appwindow-new");

    // ---- Toast 轻提示（handlers::ToastCtx，供各 handler 共享）----
    let toast_ctx = Arc::new(handlers::ToastCtx {
        weak: ui.as_weak(),
    });

    // 共享配置状态（UI 线程；退出时统一落盘）
    let shared = Rc::new(RefCell::new(cfg.clone()));

    // 初始化全局网络配置（代理 / realip），供各 reqwest client 读取
    crate::network::proxy::init(crate::network::proxy::NetOptions {
        proxy_enabled: cfg.network.proxy_enabled,
        proxy_type_index: cfg.network.proxy_type_index,
        proxy_host: cfg.network.proxy_host.clone(),
        proxy_port: cfg.network.proxy_port,
        realip_enabled: cfg.network.realip_enabled,
        realip_addr: cfg.network.realip_addr.clone(),
    });

    // 初始音量（读配置；跨播放持久）
    crate::platform::audio::set_volume(cfg.app.volume);
    ui.set_volume(cfg.app.volume);
    // 音量变化即时落盘：滑块回调同时更新配置并 save，避免异常退出丢失音量设置
    {
        let vol_shared = shared.clone();
        ui.on_volume_changed(move |v| {
            crate::platform::audio::set_volume(v);
            let mut g = vol_shared.borrow_mut();
            g.app.volume = v;
            g.save();
        });
    }

    // 设置页缓存信息初始填充（容量限制 / 占用 / 目录）
    let (cover_dir, song_dir) = {
        let g = shared.borrow();
        (
            crate::persist::config::cover_cache_dir(&g).display().to_string(),
            crate::persist::config::song_cache_dir(&g).display().to_string(),
        )
    };
    ui.set_cache_limit_mb(cfg.app.cache_limit_mb as i32);
    // 缓存占用不在启动路径同步统计（全缓存目录遍历可达几十 ms 拖慢启动），
    // 由启动后的后台统计线程 + 设置页 cache-refresh 回调填充
    ui.set_cover_dir(cover_dir.into());
    ui.set_song_dir(song_dir.into());

    // ---- 设置页多 Tab 状态初始填充（从 config.toml 恢复，见 src/config.rs）----
    ui.set_auto_launch(cfg.app.auto_launch);
    if cfg.app.auto_launch {
        // 确保自启项存在（幂等补注册）
        if let Err(e) = autolaunch::set(true) {
            log::warn!("[settings] 开机自启补注册失败: {e}");
        }
    }
    // ---- 语言（i18n）：注入可用语言列表 + 加载当前语言短码 ----
    {
        let langs = i18n::list_languages();
        let codes: Vec<slint::SharedString> = langs.iter().map(|(c, _)| c.as_str().into()).collect();
        let names: Vec<slint::SharedString> = langs.iter().map(|(_, n)| n.as_str().into()).collect();
        ui.set_language_codes(std::rc::Rc::new(slint::VecModel::from(codes.clone())).into());
        ui.set_language_options(std::rc::Rc::new(slint::VecModel::from(names)).into());
        let lang = cfg.app.language_code.clone();
        let idx = codes.iter().position(|c| c.as_str() == lang).unwrap_or(0);
        ui.set_language_index(idx as i32);
        i18n::load(&ui, &lang);
    }
    ui.set_font_index(cfg.app.font_index);
    ui.set_font_family(crate::persist::config::AppConfig::font_family(cfg.app.font_index).into());
    ui.set_quality_index(cfg.play.quality_index);
    ui.set_source_index(cfg.play.source_index);
    ui.set_auto_play(cfg.play.auto_play);
    // MV 清晰度：恢复上次选择（跨进程记忆），并同步按钮显示
    crate::handlers::mv::player::init_quality(cfg.play.mv_quality_index as u32);
    ui.set_mv_quality(cfg.play.mv_quality_index);
    // 音频输出设备：cpal WASAPI 枚举实测耗时 2~6.3s（启动慢最大元凶），
    // 移入后台线程执行，完成后回 UI 线程填充列表并恢复上次选择
    {
        let audio_ui = ui.as_weak();
        let target = cfg.play.output_device.clone();
        std::thread::spawn(move || {
            let devs = crate::platform::audio::list_devices();
            if !target.is_empty() && target != "默认设备" {
                crate::platform::audio::set_output_device(&target);
            }
            let idx = devs.iter().position(|d| *d == target).unwrap_or(0);
            let devs_ss: Vec<slint::SharedString> = devs.iter().map(|s| s.clone().into()).collect();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = audio_ui.upgrade() {
                    u.set_output_devices(slint::ModelRc::from(devs_ss.as_slice()));
                    u.set_output_index(idx as i32);
                }
            });
        });
    }
    ui.set_theme_color_index(cfg.ui.theme_color_index);
    ui.set_lyrics_style(cfg.ui.lyrics_style);
    ui.set_close_behavior_index(cfg.ui.close_behavior_index);
    ui.set_threads_index(cfg.network.threads_index);
    ui.set_buffer_index(cfg.network.buffer_index);
    // 洛雪音源脚本：注入兜底开关 + 脚本列表
    // 平台标识：web（wasm）下运行时隐藏无窗口系统 UI（标题栏窗口控制组 / 音源脚本区）。
    // .slint 无法用 #[cfg]，走"能力存在性 → 运行时 property"分支（文档 §6/§7）。
    #[cfg(target_arch = "wasm32")]
    ui.set_is_web(true);
    ui.set_lx_fallback_enabled(cfg.lx.fallback_enabled);
    ui.set_lx_auto_update(cfg.lx.auto_update);
    // 洛雪音源脚本列表注入：仅非 wasm（web 下 lx_scripts 不编译）
    #[cfg(not(target_arch = "wasm32"))]
    {
        let lx_names: Vec<slint::SharedString> = crate::network::sources::lx_scripts::list_scripts()
            .into_iter()
            .map(|s| s.into())
            .collect();
        ui.set_lx_scripts(slint::ModelRc::from(lx_names.as_slice()));
    }
    ui.set_proxy_enabled(cfg.network.proxy_enabled);
    ui.set_proxy_type(cfg.network.proxy_type_index);
    ui.set_proxy_host(cfg.network.proxy_host.clone().into());
    ui.set_proxy_port(cfg.network.proxy_port as i32);
    ui.set_realip_enabled(cfg.network.realip_enabled);
    ui.set_realip_addr(cfg.network.realip_addr.clone().into());
    ui.set_cache_enabled(cfg.cache.enabled);
    ui.set_cache_limit_text(
        (if cfg.app.cache_limit_mb == 0 { "不限制".to_string() } else { cfg.app.cache_limit_mb.to_string() }).into(),
    );
    ui.set_evict_index(cfg.cache.evict_strategy);
    // 缓存目录显示：用户设置的统一根目录（歌曲/封面/歌词子目录）
    ui.set_cache_dir(crate::persist::config::cache_root(&cfg).display().to_string().into());
    // 版本号：随 Cargo.toml 走，编译期注入，不在 UI 硬编码
    ui.set_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());

    // ---- 后端客户端与 tokio 运行时 ----
    let client = Arc::new(NeteaseClient::new());
    let rt = tokio::runtime::Runtime::new().expect("failed to init tokio runtime");
    // 封面门面（cover_warm）需要运行时句柄在 UI 线程发起后台任务
    load::set_runtime(rt.handle().clone());
    boot_mark!("client+runtime");

    // ---- 启动恢复上次播放歌曲 ----
    // 封面走统一门面：先取内存缓存（通常 miss，空图），文本信息立即可见；
    // 未命中由 cover_warm 后台补齐——splash 全屏遮挡期间完成，淡出时已就位。
    if cfg.last_song.id != 0 {
        let dur = cfg.last_song.duration_sec.max(0.0);
        let cover_url = cfg.last_song.cover_url.clone();
        let last_id = cfg.last_song.id as i32;
        ui.set_current_song(crate::Song {
            id: cfg.last_song.id as i32,
            name: cfg.last_song.name.clone().into(),
            artist: cfg.last_song.artist.clone().into(),
            album: cfg.last_song.album.clone().into(),
            cover: load::cover_now(&cover_url),
            duration: format!("{:02}:{:02}", (dur as u64) / 60, (dur as u64) % 60).into(),
            duration_sec: dur,
            url: cfg.last_song.cover_url.clone().into(),
            liked: crate::persist::liked::is_liked(cfg.last_song.id),
        });
        liked::sync_liked(&ui);
        if dur > 0.0 {
            ui.set_progress(cfg.last_song.progress_sec.clamp(0.0, dur) / dur);
        }
        let cover_ui = ui.as_weak();
        load::cover_warm(&cover_url, crate::persist::cache::CoverType::Song, move |img| {
            if let Some(u) = cover_ui.upgrade() {
                // 期间切歌则丢弃，避免旧封面覆盖新歌
                if u.get_current_song().id == last_id {
                    let mut s = u.get_current_song();
                    s.cover = img;
                    u.set_current_song(s);
                }
            }
        });
    }
    boot_mark!("restore-last-song");

    // ---- 启动异步加载首页发现页 + 歌单分类目录 ----
    rt.spawn(load::load_initial(client.clone(), ui.as_weak(), cfg.clone()));
    boot_mark!("spawn-load-initial");
    // ---- 洛雪音源内置默认：首次启动（无脚本）后台静默拉取维护清单，开箱即用。
    // 仅非 wasm（web 下 lx_scripts 不编译）----
    #[cfg(not(target_arch = "wasm32"))]
    {
        let rt_d = rt.handle().clone();
        let ui_d = ui.as_weak();
        let mut c0 = crate::persist::config::AppConfig::load();
        let need_seed = !c0.lx.seeded && c0.lx.enabled.is_empty();
        if need_seed {
            c0.lx.seeded = true;
            c0.save();
            rt_d.spawn(async move {
                let rep = crate::network::sources::lx_scripts::update_from_builtin().await;
                let names: Vec<slint::SharedString> = crate::network::sources::lx_scripts::list_scripts()
                    .into_iter()
                    .map(|s| s.into())
                    .collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ui_d.upgrade() {
                        w.set_lx_scripts(slint::ModelRc::from(names.as_slice()));
                    }
                });
                if let Ok(r) = rep {
                    log::info!("[lx] 内置默认音源初始化: 新增 {} 个, 失败 {} 个", r.added.len(), r.failed.len());
                }
            });
        }
    }
    {
        let client_cb = client.clone();
        let ui_weak = ui.as_weak();
        let rt_h = rt.handle().clone();
        rt_h.spawn(async move {
            let v = crate::network::service::playlist_catalogue(&client_cb).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_weak.upgrade() {
                    let mut names: Vec<slint::SharedString> = vec!["每日推荐".into()];
                    if let Ok(val) = v {
                        // catlist 返回 sub（子分类）与 categories（父分类），都含 name；取任一
                        let list = val.get("sub").and_then(|s| s.as_array())
                            .or_else(|| val.get("categories").and_then(|c| c.as_array()));
                        if let Some(list) = list {
                            for s in list {
                                if let Some(n) = s.get("name").and_then(|n| n.as_str()) {
                                    names.push(n.into());
                                }
                            }
                        }
                    }
                    // 接口失败/为空时兜底常见分类，避免只剩"每日推荐"
                    if names.len() <= 1 {
                        for n in ["华语", "欧美", "日韩", "流行", "摇滚", "民谣", "古风", "电子", "轻音乐", "ACG", "影视原声", "游戏"] {
                            names.push(n.into());
                        }
                    }
                    u.set_playlist_categories(Rc::new(slint::VecModel::from(names)).into());
                }
            });
        });
    }

    // ---- 装配各业务 handler（设置 / 缓存 / 搜索浏览 / 播放 / 窗口外壳）----
    let ctx = handlers::Ctx::new(
        ui.clone_strong(),
        ui.as_weak(),
        shared.clone(),
        toast_ctx.clone(),
        rt.handle().clone(),
        client.clone(),
    );
    let resolver = handlers::player::make_resolver(&ctx);
    handlers::settings::setup(&ctx);
    handlers::cache::setup(&ctx);
    handlers::search::setup(&ctx);
    handlers::player::setup(&ctx, &resolver);
    handlers::modwin::setup(&ctx, saved_pos);
    // 系统托盘（Windows）：常驻图标 + 左键显隐切换 + 右键菜单（显隐/切歌/退出）
    #[cfg(windows)]
    crate::platform::tray::init(ui.as_weak(), &cfg.app.language_code);
    handlers::browse::setup(&ctx);
    handlers::fav::setup(&ctx);
    handlers::local::setup(&ctx);
    handlers::mv::setup(&ctx);
    boot_mark!("handlers-setup");

    // ---- 启动自动播放（设置页 auto_play 开关，默认关）----
    // 延迟执行：等启动动画（splash）隐藏、内容正常显示后，再等 ~2s 才开始自动续播，
    // 避免自动播放抢在启动动画期间加载，导致启动动画卡顿/不流畅。
    if cfg.play.auto_play && cfg.last_song.id != 0 && cfg.last_song.duration_sec > 0.0 {
        let splash_ui = ui.as_weak();
        let resolver_cb = resolver.clone();
        let (lid, lname, lartist, ldur, lprog) = (
            cfg.last_song.id,
            cfg.last_song.name.clone(),
            cfg.last_song.artist.clone(),
            cfg.last_song.duration_sec,
            cfg.last_song.progress_sec.clamp(0.0, cfg.last_song.duration_sec),
        );
        let was_hidden = Rc::new(std::cell::Cell::new(false));
        let autop_timer = Rc::new(slint::Timer::default());
        let tp = autop_timer.clone();
        let wh = was_hidden.clone();
        autop_timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
            if wh.get() { return; }
            let Some(u) = splash_ui.upgrade() else { return; };
            if u.get_splash_visible() { return; }   // 启动动画尚未隐藏，继续等
            wh.set(true);
            tp.stop();
            // splash 已隐藏：再延迟 2s 触发自动播放，保证 UI 就绪且不卡启动
            let uw = splash_ui.clone();
            let rcb = resolver_cb.clone();
            let (id2, nm2, ar2, du2, pg2) = (lid, lname.clone(), lartist.clone(), ldur, lprog);
            slint::Timer::single_shot(Duration::from_millis(2000), move || {
                crate::platform::audio::begin_switch();
                if let Some(u) = uw.upgrade() { u.set_playing(true); }
                rcb(id2, nm2, ar2, du2, pg2);
            });
        });
        std::mem::forget(autop_timer);
    }

    // ---- 缓存状态统计：后台线程遍历（歌曲/歌词/封面计数 + 占用），不阻塞启动首帧 ----
    {
        let cache_ui = ui.as_weak();
        std::thread::spawn(move || {
            let usage = crate::persist::cache::total_usage();
            let usage_mb = (usage / (1024 * 1024)) as i32;
            let mc = crate::persist::cache::count_music() as i32;
            let lc = crate::persist::cache::count_lyrics() as i32;
            let cc = crate::persist::cache::count_covers() as i32;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = cache_ui.upgrade() {
                    let limit = u.get_cache_limit_mb();
                    let pct = if limit > 0 {
                        usage as f32 / (limit as f32 * 1024.0 * 1024.0)
                    } else {
                        0.0
                    };
                    u.set_cache_usage_mb(usage_mb);
                    u.set_cache_percent(pct.min(1.0));
                    u.set_music_count(mc);
                    u.set_lyric_count(lc);
                    u.set_cover_count(cc);
                }
            });
        });
    }

    // ---- 延迟显示窗口：创建时已隐藏（hook with_visible(false)），事件循环启动、
    //      首帧渲染就绪后再显示，消除启动透明空窗；splash 动画以首次 tick 为计时
    //      基准，显示时从头播放完整动画，不再被空窗期"吃掉"前几秒 ----
    {
        let show_ui = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(180), move || {
            if let Some(u) = show_ui.upgrade() {
                // 事件循环已启动后再注入窗口图标（见 set_runtime_icon 注释），
                // 然后显示窗口消除透明空窗
                set_runtime_icon(&u);
                // 启动自愈：正常模式窗口层级必须是 Normal。外部工具（自动化截图/置顶脚本等）
                // 曾把窗口推成 TOPMOST 的残留，在此统一清除；只有迷你模式才需要 AlwaysOnTop
                // （由 handlers/modwin.rs 进入迷你时设置，退出时已设回 Normal）。
                let _ = u.window().with_winit_window(|w: &slint::winit_030::winit::window::Window| {
                    w.set_window_level(slint::winit_030::winit::window::WindowLevel::Normal);
                });
                boot_mark!("show");
                let _ = u.show();
            }
        });
    }

    boot_mark!("pre-run");
    let r = ui.run();
    log::info!("[exit] 事件循环结束（run 返回），进程即将退出");
    r
}

// =====================================================================
//  web（wasm32）最小闭环入口：gdmusic 解析 + HTMLAudio 播放 + 最小 UI。
//  仅当 target_arch = wasm32（GitHub Pages / Cloudflare Pages 静态托管）编译；
//  desktop/mobile 不编译本段（走上方 main）。
//  gdmusic 开放任意 CORS → 浏览器 wasm 直接 fetch 解析（搜索/URL），无需后端。
// =====================================================================
#[cfg(target_arch = "wasm32")]
mod web_main {
    use std::rc::Rc;
    use std::time::Duration;

    use slint::ComponentHandle;
    use wasm_bindgen::prelude::*;

    // ---- gdmusic 解析已迁移 src/web/gd.rs（web 业务层）----

    #[wasm_bindgen(start)]
    pub fn run() {
        // wasm panic → console 打印定位（无 hook 时只剩 unreachable trap）
        console_error_panic_hook::set_once();
        // 存储门面（web=localStorage，zplayer/ 前缀）：settings 模块持久化依赖
        crate::platform::storage::init(std::path::PathBuf::new());
        // 同一套 appwindow.slint 骨架（wasm 编译 appwindow-web.slint，编译期嵌入 SimHei）：
        // 运行时显式设默认字体（wasm 无系统字体，否则中文空白）
        let ui = crate::AppWindow::new().expect("创建 web UI 失败");
        ui.set_is_web(true);
        ui.set_font_family("SimHei".into());
        ui.set_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
        // 音量默认 0.8（与 desktop 配置默认一致；audio/web.rs STATE 同默认值 —— 修复"没声音"根因）
        crate::platform::audio::set_volume(0.8);
        ui.set_volume(0.8);

        // 尺寸：slint web 默认窗口极小，只渲染左上角一小块。
        // 用周期 slint Timer（在 slint 事件循环内可靠触发）：wasm 加载 + slint 初始化较慢，
        // 一次性 Timer 触发时窗口未创建，set_size 会被忽略。repeat=true 每 200ms 同步一次
        // 视口（幂等），窗口创建后立即生效，直到 run() 结束。
        let _resize_timer = {
            let ui_weak = ui.as_weak();
            let timer = slint::Timer::default();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
                if let Some(u) = ui_weak.upgrade() {
                    if let Some(win) = web_sys::window() {
                        let w = win.inner_width().ok().and_then(|x| x.as_f64()).unwrap_or(1280.0) as u32;
                        let h = win.inner_height().ok().and_then(|x| x.as_f64()).unwrap_or(800.0) as u32;
                        let _ = u.window().set_size(slint::LogicalSize::new(w as f32, h as f32));
                    }
                }
            });
            timer
        };

        // 初始 set_size（run() 前尝试，窗口未创建时会被忽略，作为兜底）+ 浏览器 resize 同步
        {
            use slint::LogicalSize;
            use wasm_bindgen::JsCast;
            if let Some(win) = web_sys::window() {
                let w = win.inner_width().ok().and_then(|x| x.as_f64()).unwrap_or(1280.0) as u32;
                let h = win.inner_height().ok().and_then(|x| x.as_f64()).unwrap_or(800.0) as u32;
                let _ = ui.window().set_size(LogicalSize::new(w as f32, h as f32));

                // 浏览器 resize → 同步 slint 窗口尺寸
                let ui_weak = ui.as_weak();
                let win2 = win.clone();
                let resize = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
                    if let Some(u) = ui_weak.upgrade() {
                        let w = win2.inner_width().ok().and_then(|x| x.as_f64()).unwrap_or(1280.0) as u32;
                        let h = win2.inner_height().ok().and_then(|x| x.as_f64()).unwrap_or(800.0) as u32;
                        let _ = u.window().set_size(LogicalSize::new(w as f32, h as f32));
                    }
                }) as Box<dyn FnMut()>);
                let _ = win.add_event_listener_with_callback("resize", resize.as_ref().unchecked_ref());
                resize.forget();
            }
        }

        // ---- 无窗口系统（web）：窗口控制回调空接（按钮已由 is-web 隐藏，此处防悬空）----
        ui.on_drag_start(|_, _| {});
        ui.on_titlebar_clicked(|| {});
        ui.on_win_minimize(|| {});
        ui.on_win_mini(|| {});
        ui.on_win_maximize(|| {});
        ui.on_win_close(|| {});
        // Toast 动画结束回调：关闭 Toast（业务模块用 crate::web::show_toast 弹提示）
        {
            let ui_w = ui.as_weak();
            ui.on_toast_hidden(move || {
                if let Some(u) = ui_w.upgrade() {
                    u.set_toast_show(false);
                }
            });
        }

        // ---- 启动页时序（镜像 desktop handlers/modwin.rs：2.4s 淡出 → 0.9s 后移除）----
        {
            let ui_w = ui.as_weak();
            let timer = Rc::new(slint::Timer::default());
            let t2 = timer.clone();
            // wasm32-unknown-unknown 无 std::time::Instant（调用即 panic "time not implemented on this platform"），
            // 用周期 tick 计数推演时间（Timer 每 50ms 触发一次，等价 Instant 语义）
            let tick = Rc::new(std::cell::Cell::new(0u32));
            let tk = tick.clone();
            let fade_at = Rc::new(std::cell::Cell::new(0.0f32));
            let fa = fade_at.clone();
            timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
                let Some(u) = ui_w.upgrade() else { return };
                tk.set(tk.get() + 1);
                let elapsed = tk.get() as f32 * 0.05;
                u.set_splash_t(elapsed);
                u.set_splash_loader(((elapsed - 0.8) / 1.0).clamp(0.0, 1.0));
                if fa.get() == 0.0 {
                    if elapsed >= 2.4 {
                        u.set_splash_fade(true);
                        fa.set(elapsed);
                    }
                } else if elapsed - fa.get() >= 0.9 {
                    u.set_splash_visible(false);
                    t2.stop();
                }
            });
            std::mem::forget(timer);
        }

        // ---- 业务模块装配（gdmusic 数据通道 + 各页回调接线；编排见 crate::web::setup_all）----
        crate::web::setup_all(&ui);

        // slint web backend 需要 run() 驱动渲染循环（winit web 用异常控制流作为帧信号，
        // 由 slint 的 web backend 内部 catch 处理）；不能用 show()（只显示不驱动 → 黑屏）。
        ui.run().expect("web 事件循环失败");
    }
}
