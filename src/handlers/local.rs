//! 本地资源页：扫描本地音频/视频文件，填充歌曲与视频列表，支持搜索过滤。
//! 本地视频点击用 MV 播放器播放（file:// 直连，不走代理）。

use super::Ctx;
use crate::{AppWindow, Song};
use slint::{ComponentHandle, VecModel};
use std::rc::Rc;
use std::sync::Mutex;

/// 全量扫描结果（未过滤的原始 JSON），搜索时基于此过滤。
/// 用 serde_json::Value 而非 Song：Song 含 Image 字段不实现 Sync，
/// 不能放入 static Mutex。
static FULL_SONGS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());
static FULL_VIDEOS: Mutex<Vec<serde_json::Value>> = Mutex::new(Vec::new());

/// 把 serde_json 的扫描结果转为 Song 模型。
fn to_song(v: &serde_json::Value, idx: usize) -> Song {
    let name = v["name"].as_str().unwrap_or("").to_string();
    let artist = v["artist"].as_str().unwrap_or("").to_string();
    let album = v["album"].as_str().unwrap_or("").to_string();
    let path = v["path"].as_str().unwrap_or("").to_string();
    let format = v["format"].as_str().unwrap_or("").to_string();
    let duration_sec = v["duration"].as_f64().unwrap_or(0.0) as f32;
    let duration = if duration_sec > 0.0 {
        let m = (duration_sec as u32) / 60;
        let s = (duration_sec as u32) % 60;
        format!("{:02}:{:02}", m, s)
    } else {
        format.to_uppercase()
    };
    Song {
        id: idx as i32,
        name: name.into(),
        artist: artist.into(),
        album: album.into(),
        cover: crate::load::cover_now(""),
        duration: duration.into(),
        duration_sec,
        // url 存本地文件绝对路径，播放时判断 local 标记走本地读取
        url: path.into(),
        liked: false,
    }
}

/// 把视频扫描结果转为 Song 模型（复用 Song 结构承载视频行）。
fn to_video(v: &serde_json::Value, idx: usize) -> Song {
    let name = v["name"].as_str().unwrap_or("").to_string();
    let path = v["path"].as_str().unwrap_or("").to_string();
    let format = v["format"].as_str().unwrap_or("").to_string();
    let size = v["size"].as_u64().unwrap_or(0);
    let size_str = if size > 0 {
        let mb = size as f64 / 1048576.0;
        format!("{:.1}MB", mb)
    } else {
        String::new()
    };
    Song {
        id: idx as i32,
        name: name.into(),
        artist: format.to_uppercase().into(),
        album: path.clone().into(),
        cover: crate::load::cover_now(""),
        duration: size_str.into(),
        duration_sec: 0.0,
        url: path.into(),
        liked: false,
    }
}

/// 扫描本地目录，填充歌曲与视频列表。
pub(crate) fn fill_local(u: &AppWindow) {
    let dir = crate::persist::config::local_dir();
    let dir_str = dir.to_string_lossy().to_string();
    match crate::network::netease::local::scan_local_resources(&dir_str, true) {
        Ok((songs, videos)) => {
            *FULL_SONGS.lock().unwrap() = songs;
            *FULL_VIDEOS.lock().unwrap() = videos;
            apply_filter(u);
        }
        Err(e) => {
            log::warn!(target: "local", "scan local resources failed: {e}");
            *FULL_SONGS.lock().unwrap() = Vec::new();
            *FULL_VIDEOS.lock().unwrap() = Vec::new();
            apply_filter(u);
        }
    }
}

/// 判断一条原始 JSON 是否匹配搜索文本。
fn matches_query(v: &serde_json::Value, q: &str) -> bool {
    if q.is_empty() { return true; }
    let name = v["name"].as_str().unwrap_or("").to_lowercase();
    let artist = v["artist"].as_str().unwrap_or("").to_lowercase();
    let album = v["album"].as_str().unwrap_or("").to_lowercase();
    name.contains(q) || artist.contains(q) || album.contains(q)
}

/// 按搜索文本过滤全量列表并写入 UI。
fn apply_filter(u: &AppWindow) {
    let q = u.get_local_search_text().to_string().to_lowercase();
    let songs = FULL_SONGS.lock().unwrap().clone();
    let videos = FULL_VIDEOS.lock().unwrap().clone();
    let filtered_songs: Vec<Song> = songs
        .iter()
        .enumerate()
        .filter(|(_, v)| matches_query(v, &q))
        .map(|(i, v)| to_song(v, i))
        .collect();
    let filtered_videos: Vec<Song> = videos
        .iter()
        .enumerate()
        .filter(|(_, v)| matches_query(v, &q))
        .map(|(i, v)| to_video(v, i))
        .collect();
    u.set_local_song_list(Rc::new(VecModel::from(filtered_songs)).into());
    u.set_local_video_list(Rc::new(VecModel::from(filtered_videos)).into());
}

pub fn setup(ctx: &Ctx) {
    // 页面切入填充：9=本地资源
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_page_fill(move |nav| {
        if nav == 9 {
            if let Some(u) = ui_weak.upgrade() {
                fill_local(&u);
            }
        }
    });

    // 搜索文本变化：实时过滤
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_local_search_changed(move |text| {
        if let Some(u) = ui_weak.upgrade() {
            u.set_local_search_text(text);
            apply_filter(&u);
        }
    });

    // 分类切换：数据已预填，无需 Rust 侧操作
    ctx.app.clone_strong().on_local_tab_changed(move |idx| {
        log::info!(target: "local", "tab changed to {idx}");
    });

    // 播放本地视频：用 MV 播放器播放本地文件（file:// 直连）
    let ui_weak = ctx.ui.clone();
    let client_pf = ctx.client.clone();
    let rt_pf = ctx.rt.clone();
    ctx.app.clone_strong().on_local_play_video(move |id| {
        let Some(_u) = ui_weak.upgrade() else { return };
        let videos = FULL_VIDEOS.lock().unwrap().clone();
        let Some(v) = videos.get(id as usize) else { return };
        let path = v["path"].as_str().unwrap_or("").to_string();
        let name = v["name"].as_str().unwrap_or("").to_string();
        if path.is_empty() { return; }
        // 复用 MV 播放器播放本地视频：MvPlayer.start_playback 接受本地路径
        // （非 http(s) 不走代理，ffmpeg 直读文件）。
        crate::handlers::mv::player::open_local_video(
            &ui_weak,
            &client_pf,
            &rt_pf,
            path,
            name,
        );
    });
}
