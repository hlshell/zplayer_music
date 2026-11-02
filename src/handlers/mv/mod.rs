//! MV 播放器 handler 入口。
//! 全局注册 open/close/play-pause/seek/next/prev/quality 等回调。

use slint::ComponentHandle;

pub mod player;
pub mod queue;
pub mod search;
pub mod window;

/// 注册 AppWindow 上的 MV 相关 callback（由 main.rs 的 handlers 初始化序列调用）。
pub fn setup(ctx: &super::Ctx) {
    let ui_weak = ctx.ui.clone();

    // ---- back：关闭全屏播放器，Rust 侧停掉解码线程 ----
    ctx.app.clone_strong().on_mv_back({
        let ui = ui_weak.clone();
        move || {
            // 画中画态先还原窗口（否则窗口残留小尺寸 + 置顶）
            if let Some(u) = ui.upgrade() {
                if u.get_mv_compact() {
                    window::exit_compact(&ui);
                }
                // 先置 false 让 UI 消失，再停解码（stop_playback 等线程退出）
                u.set_mv_player_visible(false);
            }
            window::exit_fullscreen(&ui);
            player::close_mv_player();
        }
    });

    // ---- play-pause ----
    ctx.app.clone_strong().on_mv_play_pause({
        let ui = ui_weak.clone();
        move || {
            player::toggle_play_pause();
            if let Some(u) = ui.upgrade() {
                let now = u.get_mv_playing();
                u.set_mv_playing(!now);
            }
        }
    });

    // ---- seek ----
    ctx.app.clone_strong().on_mv_seek(move |ms| {
        player::seek_to_ms(ms as i64);
    });

    // ---- prev / next（MV 队列切换）----
    ctx.app.clone_strong().on_mv_prev({
        let ui = ui_weak.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        move || player::play_neighbor(&ui, &client, &rt, false)
    });
    ctx.app.clone_strong().on_mv_next({
        let ui = ui_weak.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        move || player::play_neighbor(&ui, &client, &rt, true)
    });

    // ---- 清晰度切换：记忆选择 + 原地重开当前 MV 续播 ----
    ctx.app.clone_strong().on_mv_quality_changed({
        let ui = ui_weak.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        move |idx| player::quality_changed(&ui, &client, &rt, idx)
    });

    // ---- 音量 / 静音 ----
    ctx.app.clone_strong().on_mv_set_volume({
        let ui = ui_weak.clone();
        move |v| player::set_volume_ui(&ui, v)
    });
    ctx.app.clone_strong().on_mv_mute_toggle({
        let ui = ui_weak.clone();
        move || player::mute_toggle_ui(&ui)
    });

    // ---- 全屏切换 ----
    ctx.app.clone_strong().on_mv_toggle_fullscreen({
        let ui = ui_weak.clone();
        move || window::toggle_fullscreen(&ui)
    });

    // ---- 画中画（迷你窗口）切换 ----
    ctx.app.clone_strong().on_mv_compact_toggle({
        let ui = ui_weak.clone();
        move || window::toggle_compact(&ui)
    });

    // ---- 播放列表抽屉点选 ----
    ctx.app.clone_strong().on_mv_playlist_play({
        let ui = ui_weak.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        move |id| player::play_id(&ui, &client, &rt, id)
    });

    // ---- 抽屉内搜索 ----
    search::setup(ctx);

    // ---- 本地视频拖拽播放（仅本地资源入口打开的视频播放器支持）----
    // 拖入视频文件时用 open_local_video 播放；drop_files::set_enabled 在
    // open_local_video/close_mv_player 中切换，确保仅本地视频播放器打开时生效。
    #[cfg(target_os = "windows")]
    {
        let ui_drop = ui_weak.clone();
        let client_drop = ctx.client.clone();
        let rt_drop = ctx.rt.clone();
        crate::platform::drop_files::set_callback(move |paths: Vec<String>| {
            if let Some(path) = paths.first() {
                let name = std::path::Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("本地视频")
                    .to_string();
                player::open_local_video(&ui_drop, &client_drop, &rt_drop, path.clone(), name);
            }
        });
    }
}
