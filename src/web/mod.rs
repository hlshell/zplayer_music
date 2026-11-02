//! web(wasm32) 业务层：gdmusic 数据通道（gd）+ 按业务拆分的回调接线模块。
//! 每个业务模块导出 setup(ui: &AppWindow) 入口，web_main 启动编排统一调用（main.rs 只留编排）。
//! 回调归属（appwindow.slint root，每回调仅一个模块接线，否则后接覆盖前接）：
//!   search.rs  : search-text-changed / search-clicked
//!   play.rs    : play-song / play-toggle / prev-song / next-song / play-mode-changed / seek /
//!                volume-changed / like-toggle / like-toggle-id / clear-queue / remove-song /
//!                request-lyrics / hero-play-all / detail-play-all / locate-playing / mini-queue-toggle
//!   browse.rs  : open-daily-detail / open-resource-detail / detail-back / detail-search-changed /
//!                detail-like-toggle / playlist-cat-clicked / album-cat-clicked / radio-cat-clicked /
//!                liked-unlike / recent-remove / recent-clear / page-fill / nav-anim
//!   settings.rs: theme-toggle / settings-select-changed / settings-toggle-changed /
//!                settings-input-changed / settings-proxy-submit / settings-clicked /
//!                settings-clear-cache / settings-check-update / settings-donate-clicked /
//!                cache-limit-clicked / cover-dir-clicked / song-dir-clicked / cache-refresh
//!   web_main   : drag-start / titlebar-clicked / win-minimize / win-mini / win-maximize /
//!                win-close / toast-hidden（无窗口系统空接 + Toast 关闭）

pub mod browse;
pub mod gd;
pub mod play;
pub mod search;
pub mod settings;

/// Toast 轻提示（web 无桌面 ToastCtx；各业务模块共用本函数，web_main 接线 toast-hidden 关闭）。
pub fn show_toast(ui: &crate::AppWindow, text: &str, kind: &str) {
    ui.set_toast_text(text.into());
    ui.set_toast_kind(kind.into());
    ui.set_toast_show(true);
}

/// 装配所有 web 业务模块（web_main 启动编排最后调用）。
pub fn setup_all(ui: &crate::AppWindow) {
    // 先恢复持久化状态（收藏/最近播放重登记；队列恢复在 play::setup 内）
    gd::load_persisted();
    search::setup(ui);
    play::setup(ui);
    browse::setup(ui);
    settings::setup(ui);
}
