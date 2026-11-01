//! 设置页回调：7 个 Tab 的 选择 / 开关 / 输入 / 弹窗提交 / 关于 按钮，
//! 统一读写共享配置（config.toml 持久化，见 src/config.rs）并联动各模块。

use super::Ctx;
use crate::platform::autolaunch;
use crate::network::proxy;
use crate::persist::state;
use slint::ComponentHandle;
use slint::Model;

pub fn setup(ctx: &Ctx) {
    // 深浅主题切换持久化（切换由 UI 侧 theme-toggle 翻转 dark，这里只负责持久化）
    let ui = ctx.ui.clone();
    let shared_th = ctx.shared.clone();
    ctx.app.clone_strong().on_theme_toggle(move || {
        let Some(u) = ui.upgrade() else { return; };
        shared_th.borrow_mut().app.dark = u.get_dark();
    });

    // 设置页多 Tab：选择（下拉/单选）持久化
    let ui = ctx.ui.clone();
    let shared_sel = ctx.shared.clone();
    ctx.app.clone_strong().on_settings_select_changed(move |id, idx| {
        let Some(u) = ui.upgrade() else { return; };
        let mut c = shared_sel.borrow_mut();
        match id {
            1 => c.app.dark = idx == 1,
            2 => {
                // 语言：idx → 短码（language-codes 列表），存配置并立即加载
                let codes = u.get_language_codes();
                let code = codes.row_data(idx as usize).unwrap_or_else(|| "zh".into());
                c.app.language_code = code.to_string();
                u.set_language_code(code.clone());
                u.set_language_index(idx);
                crate::i18n::load(&u, &c.app.language_code);
            }
            3 => {
                c.app.font_index = idx;
                u.set_font_family(crate::persist::config::AppConfig::font_family(idx).into());
            }
            4 => c.play.quality_index = idx,
            5 => c.play.source_index = idx,
            6 => {
                // 音频输出设备：真实切换（枚举设备 → 重建输出流）
                let devs = crate::platform::audio::list_devices();
                let name = devs.get(idx as usize).cloned().unwrap_or_else(|| "默认设备".to_string());
                c.play.output_index = idx;
                c.play.output_device = name.clone();
                let ok = crate::platform::audio::set_output_device(&name);
                log::info!("[settings] 输出设备 → {} ({}）", name, if ok { "ok" } else { "failed" });
            }
            7 => c.ui.theme_color_index = idx,
            8 => c.ui.close_behavior_index = idx,
            9 => c.network.threads_index = idx,
            10 => c.network.buffer_index = idx,
            13 => c.cache.evict_strategy = idx,
            14 => c.ui.lyrics_style = idx == 0,
            _ => {}
        }
        c.save();
        drop(c);
        u.set_dark(shared_sel.borrow().app.dark);
        u.set_lyrics_style(shared_sel.borrow().ui.lyrics_style);
        crate::persist::cache::init(&shared_sel.borrow());
        log::info!("[settings] select id={} idx={}", id, idx);
    });

    // 设置页多 Tab：开关（开机自启 / 自动播放 / 缓存开关 / realip / 代理）
    let ui = ctx.ui.clone();
    let shared_tg = ctx.shared.clone();
    ctx.app.clone_strong().on_settings_toggle_changed(move |id, on| {
        let Some(u) = ui.upgrade() else { return; };
        let mut c = shared_tg.borrow_mut();
        match id {
            21 => {
                c.app.auto_launch = on;
                if let Err(e) = autolaunch::set(on) {
                    log::warn!("[settings] 开机自启写入失败: {e}");
                }
            }
            22 => c.play.auto_play = on,
            23 => c.cache.enabled = on,
            24 => c.network.realip_enabled = on,
            25 => c.network.proxy_enabled = on,
            26 => c.lx.fallback_enabled = on,
            27 => c.lx.auto_update = on,
            _ => {}
        }
        c.save();
        log::info!("[settings] toggle id={} on={}", id, on);
        if id == 23 {
            state::refresh_cache_stats(&u);
        }
        // 网络相关开关变更 → 同步全局网络配置
        if id == 24 || id == 25 {
            proxy::update(proxy::NetOptions {
                proxy_enabled: c.network.proxy_enabled,
                proxy_type_index: c.network.proxy_type_index,
                proxy_host: c.network.proxy_host.clone(),
                proxy_port: c.network.proxy_port,
                realip_enabled: c.network.realip_enabled,
                realip_addr: c.network.realip_addr.clone(),
            });
        }
    });

    // 设置页多 Tab：输入（缓存上限 / realip 地址）
    let ui = ctx.ui.clone();
    let shared_in = ctx.shared.clone();
    ctx.app.clone_strong().on_settings_input_changed(move |id, text| {
        let Some(u) = ui.upgrade() else { return; };
        if id == 31 {
            let parsed = text.trim().parse::<u64>().unwrap_or(4096);
            let mut c = shared_in.borrow_mut();
            c.app.cache_limit_mb = parsed;
            c.save();
            drop(c);
            u.set_cache_limit_mb(parsed as i32);
            u.set_cache_limit_text(parsed.to_string().into());
            crate::persist::cache::init(&shared_in.borrow());
            crate::persist::cache::enforce_limit_now();
            state::refresh_cache_stats(&u);
            log::info!("[settings] 缓存上限 → {}MB", parsed);
        } else if id == 51 {
            // realip 手动地址
            let mut c = shared_in.borrow_mut();
            c.network.realip_addr = text.trim().to_string();
            c.save();
            proxy::update(proxy::NetOptions {
                proxy_enabled: c.network.proxy_enabled,
                proxy_type_index: c.network.proxy_type_index,
                proxy_host: c.network.proxy_host.clone(),
                proxy_port: c.network.proxy_port,
                realip_enabled: c.network.realip_enabled,
                realip_addr: c.network.realip_addr.clone(),
            });
            log::info!("[settings] realip 地址 → {}", text.trim());
        }
    });

    // 设置页多 Tab：跳转按钮（缓存根目录 / 快捷键占位）
    let ui = ctx.ui.clone();
    let shared_cd = ctx.shared.clone();
    let toast_cd = ctx.toast.clone();
    let rt_cd = ctx.rt.clone();
    let ui_cd = ctx.ui.clone();
    ctx.app.clone_strong().on_settings_clicked(move |id| {
        let Some(u) = ui.upgrade() else { return; };
        match id {
            // 41/44/45 依赖 rfd(桌面文件对话框) 或 lx_scripts(洛雪脚本)：web 下不注册
            #[cfg(not(target_arch = "wasm32"))]
            41 => {
                // 缓存根目录（歌曲/封面/歌词统一存放其下子目录）
                if let Some(s) = tinyfiledialogs::select_folder_dialog("选择缓存根目录", "") {
                    let mut c = shared_cd.borrow_mut();
                    c.cache.dir = s.clone();
                    c.cache.cover_dir = String::new();
                    c.cache.song_dir = String::new();
                    c.save();
                    drop(c);
                    crate::persist::cache::init(&shared_cd.borrow());
                    u.set_cache_dir(s.clone().into());
                    state::refresh_cache_stats(&u);
                    log::info!("[settings] 缓存根目录 → {}", s);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            45 => {
                // 检查更新音源脚本：从内置维护清单拉取并校验
                toast_cd.show("正在检查音源更新…", "info");
                let rt_job = rt_cd.clone();
                let ui_job = ui_cd.clone();
                let toast_job = toast_cd.clone();
                rt_job.spawn(async move {
                    match crate::network::sources::lx_scripts::update_from_builtin().await {
                        Ok(rep) => {
                            let names: Vec<slint::SharedString> =
                                crate::network::sources::lx_scripts::list_scripts()
                                    .into_iter()
                                    .map(|s| s.into())
                                    .collect();
                            let msg = if rep.added.is_empty() {
                                format!("已是最新（{} 个内置源暂不可用）", rep.failed.len())
                            } else {
                                format!("已更新 {} 个音源：{}", rep.added.len(), rep.added.join(", "))
                            };
                            let msg_clone = msg.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ui_job.upgrade() {
                                    w.set_lx_scripts(slint::ModelRc::from(names.as_slice()));
                                }
                                toast_job.show(&msg_clone, if rep.added.is_empty() { "info" } else { "success" });
                            });
                            log::info!("[settings] 音源更新完成: {msg}");
                        }
                        Err(e) => {
                            log::warn!("[settings] 音源更新失败: {e}");
                            let _ = slint::invoke_from_event_loop(move || toast_job.show(&e, "error"));
                        }
                    }
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            44 => {
                // 导入洛雪音源脚本：选择 .js → 复制到脚本目录 → 校验 → 加入启用列表
                if let Some(s) = tinyfiledialogs::open_file_dialog("导入洛雪音源脚本", "", Some((&["*.js"], ""))) {
                    let p = std::path::PathBuf::from(s);
                    match crate::network::sources::lx_scripts::import_script(&p) {
                        Ok(name) => {
                            let mut c = shared_cd.borrow_mut();
                            if !c.lx.enabled.contains(&name) {
                                c.lx.enabled.push(name.clone());
                            }
                            c.save();
                            drop(c);
                            let names: Vec<slint::SharedString> =
                                crate::network::sources::lx_scripts::list_scripts()
                                    .into_iter()
                                    .map(|s| s.into())
                                    .collect();
                            u.set_lx_scripts(slint::ModelRc::from(names.as_slice()));
                            toast_cd.show(&format!("已导入音源脚本：{name}"), "success");
                            log::info!("[settings] 导入洛雪音源脚本 → {name}");
                        }
                        Err(e) => {
                            toast_cd.show(&e, "error");
                            log::warn!("[settings] 导入洛雪音源脚本失败: {e}");
                        }
                    }
                }
            }
            42 => {
                toast_cd.show("快捷键设置开发中", "info");
            }
            _ => {}
        }
    });

    // 代理配置提交：类型 + host + port（port 非法时置 0 忽略）
    let shared_px = ctx.shared.clone();
    ctx.app.clone_strong().on_settings_proxy_submit(move |typ, host, port| {
        let mut c = shared_px.borrow_mut();
        c.network.proxy_type_index = typ.clamp(0, 3);
        c.network.proxy_host = host.trim().to_string();
        c.network.proxy_port = port.trim().parse::<u16>().unwrap_or(0);
        c.save();
        proxy::update(proxy::NetOptions {
            proxy_enabled: c.network.proxy_enabled,
            proxy_type_index: c.network.proxy_type_index,
            proxy_host: c.network.proxy_host.clone(),
            proxy_port: c.network.proxy_port,
            realip_enabled: c.network.realip_enabled,
            realip_addr: c.network.realip_addr.clone(),
        });
        log::info!("[settings] 代理(type={}) → {}:{}", typ, c.network.proxy_host, c.network.proxy_port);
    });

    // 关于：检查更新 / 捐赠（占位提示）
    let t_check = ctx.toast.clone();
    ctx.app.clone_strong().on_settings_check_update(move || {
        t_check.show("已是最新版本", "info");
    });
    let t_donate = ctx.toast.clone();
    ctx.app.clone_strong().on_settings_donate_clicked(move || {
        t_donate.show("感谢支持！捐赠功能开发中", "info");
    });
}
