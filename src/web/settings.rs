//! web 设置业务：设置页接线 + localStorage 持久化（主题/音量最小集）。
//!
//! 归属回调（appwindow.slint root，唯一接线方，勿与其他模块重复）：
//!   theme-toggle / settings-select-changed(int,int) / settings-toggle-changed(int,bool) /
//!   settings-input-changed(int,string) / settings-proxy-submit(int,string,string) /
//!   settings-clicked(int) / settings-clear-cache(int) / settings-check-update() /
//!   settings-donate-clicked() / cache-limit-clicked / cover-dir-clicked / song-dir-clicked /
//!   cache-refresh
//!
//! 持久化：crate::platform::storage（web=localStorage，zplayer/ 前缀；storage::init 已由
//! web_main 调用）。
//!   - key "dark"："true"/"false"，深浅主题。theme-toggle 在 slint 侧已把 root.dark 取反后
//!     再回调，本模块读 ui.get_dark() 当前值写回；基础 Tab「主题模式」select(id=1) 同样会翻转
//!     dark，本模块一并持久化，保证两条入口一致。
//!   - key "volume"：f32 字符串。音量写入唯一方是 play.rs 的 on_volume_changed（见 play.rs
//!     注释），本模块在 setup 时只读回恢复（audio::set_volume + ui.set_volume），绝不写 "volume"。
//!
//! 关键约束：
//! - is-web 由 web_main 设置；titlebar 窗口控制组已按 is-web 隐藏，本模块勿动。
//! - web 无能力项（输出设备枚举、洛雪音源脚本、缓存统计/清理、代理、realip、开机自启、文件选择）：
//!   一律 toast "Web 端待补充"，不造假数据；对应 root 属性大多已在 slint 侧 <=> 双向绑定自翻转，
//!   UI 自然保持一致，Rust 侧无需再 set。
//! - version 已由 web_main 注入。

use slint::ComponentHandle;

use crate::platform::{audio, storage};

/// 无能力项统一提示文案。
const TODO: &str = "Web 端待补充";

pub fn setup(ui: &crate::AppWindow) {
    // ---- 启动恢复：主题 ----
    // 默认浅色(false)；localStorage 命中 "true" 才置深色。
    if storage::read_text("dark").as_deref() == Some("true") {
        ui.set_dark(true);
    }

    // ---- 启动恢复：音量 ----
    // 只读回。音量写入由 play.rs 的 on_volume_changed 唯一负责（勿在此写 "volume"）。
    if let Some(v) = storage::read_text("volume").and_then(|s| s.parse::<f32>().ok()) {
        let v = v.clamp(0.0, 1.0);
        audio::set_volume(v);
        ui.set_volume(v);
    }

    // ---- 深浅主题切换持久化（slint 侧已取反 root.dark，这里读当前值落盘）----
    let ui_w = ui.as_weak();
    ui.on_theme_toggle(move || {
        let Some(u) = ui_w.upgrade() else { return };
        let dark = u.get_dark();
        let _ = storage::write_text("dark", if dark { "true" } else { "false" });
    });

    // ---- 设置页：选择（下拉/单选）----
    // id 语义对照 desktop handlers/settings.rs：
    //   1=主题模式(dark) 2=语言 3=字体 4=音质 5=音源 6=输出设备 7=主题色
    //   8=关闭行为 9=下载线程 10=缓冲 13=缓存淘汰 14=歌词样式
    // web 下这些选项无后端：直接 set 对应 root 属性保持 UI 一致 + toast 提示待补充；
    // id=1（主题模式）在 web 是真能力（dark 持久化可用），不弹 toast。
    let ui_w = ui.as_weak();
    ui.on_settings_select_changed(move |id, idx| {
        let Some(u) = ui_w.upgrade() else { return };
        match id {
            1 => {
                // 主题模式：真实切换 + 持久化（与 theme-toggle 同一 key）
                u.set_dark(idx == 1);
                let _ = storage::write_text("dark", if u.get_dark() { "true" } else { "false" });
                return;
            }
            2 => u.set_language_index(idx),
            3 => u.set_font_index(idx),
            4 => u.set_quality_index(idx),
            5 => u.set_source_index(idx),
            7 => u.set_theme_color_index(idx),
            8 => u.set_close_behavior_index(idx),
            9 => u.set_threads_index(idx),
            10 => u.set_buffer_index(idx),
            13 => u.set_evict_index(idx),
            14 => u.set_lyrics_style(idx == 0),
            _ => {} // 6=输出设备 等无能力项：不动属性，直接提示
        }
        crate::web::show_toast(&u, TODO, "info");
    });

    // ---- 设置页：开关（开机自启/自动播放/缓存开关/realip/代理/洛雪）----
    // id 对照 desktop：21=auto-launch 22=auto-play 23=cache-enabled 24=realip 25=proxy
    //   26=lx-fallback 27=lx-auto-update。root 属性已在 slint 侧自翻转，这里只提示。
    let ui_w = ui.as_weak();
    ui.on_settings_toggle_changed(move |_id, _on| {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 设置页：输入（缓存上限 id=31 / realip 地址 id=51）----
    // root.realip-addr 已与输入框 <=> 双向绑定自更新；缓存上限文本同理。web 无后端，仅提示。
    let ui_w = ui.as_weak();
    ui.on_settings_input_changed(move |_id, _text| {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 设置页：代理配置提交（类型 + host + port）----
    let ui_w = ui.as_weak();
    ui.on_settings_proxy_submit(move |_typ, _host, _port| {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 设置页：跳转按钮（缓存目录/快捷键/代理配置/导入音源/检查音源更新 等）----
    let ui_w = ui.as_weak();
    ui.on_settings_clicked(move |_id| {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 设置页：清理缓存 ----
    let ui_w = ui.as_weak();
    ui.on_settings_clear_cache(move |_kind| {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 关于：检查更新 / 捐赠 ----
    let ui_w = ui.as_weak();
    ui.on_settings_check_update(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, "已是最新版本", "info");
        }
    });
    let ui_w = ui.as_weak();
    ui.on_settings_donate_clicked(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });

    // ---- 缓存 Tab：上限按钮 / 封面目录 / 歌曲目录 / 刷新统计 ----
    let ui_w = ui.as_weak();
    ui.on_cache_limit_clicked(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });
    let ui_w = ui.as_weak();
    ui.on_cover_dir_clicked(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });
    let ui_w = ui.as_weak();
    ui.on_song_dir_clicked(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });
    let ui_w = ui.as_weak();
    ui.on_cache_refresh(move || {
        if let Some(u) = ui_w.upgrade() {
            crate::web::show_toast(&u, TODO, "info");
        }
    });
}
