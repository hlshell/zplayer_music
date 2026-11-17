//! 本地资源页：扫描本地音频/视频文件，填充歌曲与视频列表，支持搜索过滤。
//! 本地视频点击用 MV 播放器播放（file:// 直连，不走代理）。
//!
//! 性能约定：
//! - 目录扫描 + 音频标签解析全部在 `spawn_blocking` 后台线程执行，UI 线程零阻塞；
//! - 扫描结果转成轻量 `Entry`（预存小写检索串），输入过滤时不再解析 JSON / 重复分配；
//! - 同一目录只扫描一次（切换页面不重复扫描），用户可手动「重新扫描」或改选目录。

use super::Ctx;
use crate::{AppWindow, Song};
use slint::{ComponentHandle, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Song,
    Video,
}

/// 扫描产出的轻量行（纯 Rust，Send）：搜索/建模型时不再回头解析 JSON。
struct Entry {
    kind: Kind,
    name: String,
    artist: String,
    album: String,
    path: String,
    format: String,
    size: u64,
    duration_sec: f64,
    /// name/artist/album 的小写拼接串，过滤时直接 contains
    hay: String,
}

impl Entry {
    fn from_json(v: &serde_json::Value, kind: Kind) -> Self {
        let name = v["name"].as_str().unwrap_or("").to_string();
        let artist = v["artist"].as_str().unwrap_or("").to_string();
        let album = v["album"].as_str().unwrap_or("").to_string();
        let hay = format!("{name}\u{1}{artist}\u{1}{album}").to_lowercase();
        Self {
            kind,
            name,
            artist,
            album,
            path: v["path"].as_str().unwrap_or("").to_string(),
            format: v["format"].as_str().unwrap_or("").to_string(),
            size: v["size"].as_u64().unwrap_or(0),
            duration_sec: v["duration"].as_f64().unwrap_or(0.0),
            hay,
        }
    }

    fn matches(&self, q: &str) -> bool {
        self.hay.contains(q)
    }

    fn to_song(&self, id: i32) -> Song {
        let duration = if self.duration_sec > 0.0 {
            fmt_duration(self.duration_sec)
        } else {
            self.format.to_uppercase()
        };
        Song {
            id,
            name: self.name.clone().into(),
            artist: self.artist.clone().into(),
            album: self.album.clone().into(),
            cover: crate::load::cover_now(""),
            duration: duration.into(),
            duration_sec: self.duration_sec as f32,
            // url 存本地文件绝对路径，播放时判断 local 标记走本地读取
            url: self.path.clone().into(),
            liked: false,
        }
    }

    fn to_video(&self, id: i32) -> Song {
        let size_str = if self.size > 0 {
            format!("{:.1}MB", self.size as f64 / 1048576.0)
        } else {
            String::new()
        };
        Song {
            id,
            name: self.name.clone().into(),
            artist: self.format.to_uppercase().into(),
            album: self.path.clone().into(),
            cover: crate::load::cover_now(""),
            duration: size_str.into(),
            duration_sec: 0.0,
            url: self.path.clone().into(),
            liked: false,
        }
    }
}

/// 全量扫描结果（未过滤）。id 为条目在全集中的下标，播放时据此取路径。
static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
/// 最近一次成功扫描的目录（同目录切页不重复扫描）。
static SCANNED_DIR: Mutex<String> = Mutex::new(String::new());
/// 扫描进行中门闩（防止切页/连点并发扫描）。
static SCANNING: AtomicBool = AtomicBool::new(false);
/// 扫描期间又被请求的目录（如扫描中改选目录）：本轮结束后自动补扫。
static PENDING_DIR: Mutex<Option<String>> = Mutex::new(None);

fn fmt_duration(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn fmt_total(sec: f64) -> String {
    let s = sec.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// 按搜索文本过滤全量列表并写入 UI（统计卡展示全量数据，不随过滤变化）。
fn apply_filter(u: &AppWindow) {
    let q = u.get_local_search_text().to_string().to_lowercase();
    let (songs, videos) = {
        let entries = ENTRIES.lock().unwrap();
        let mut songs = Vec::new();
        let mut videos = Vec::new();
        for (idx, e) in entries.iter().enumerate() {
            if !q.is_empty() && !e.matches(&q) {
                continue;
            }
            match e.kind {
                Kind::Song => songs.push(e.to_song(idx as i32)),
                Kind::Video => videos.push(e.to_video(idx as i32)),
            }
        }
        (songs, videos)
    };
    u.set_local_song_list(Rc::new(VecModel::from(songs)).into());
    u.set_local_video_list(Rc::new(VecModel::from(videos)).into());
}

/// 统计卡：歌曲数 / 视频数 / 专辑数 / 总时长（全量）。
fn set_stats(u: &AppWindow, entries: &[Entry]) {
    let mut albums = std::collections::HashSet::new();
    let (mut songs, mut videos, mut total) = (0, 0, 0.0);
    for e in entries {
        match e.kind {
            Kind::Song => {
                songs += 1;
                total += e.duration_sec;
                if !e.album.is_empty() {
                    albums.insert(e.album.to_lowercase());
                }
            }
            Kind::Video => videos += 1,
        }
    }
    u.set_local_song_count(songs);
    u.set_local_video_count(videos);
    u.set_local_album_count(albums.len() as i32);
    u.set_local_total_duration(fmt_total(total).into());
}

/// 后台扫描指定目录；完成后回 UI 线程建模型。同一时刻只允许一个扫描任务。
fn start_scan(ui: slint::Weak<AppWindow>, rt: tokio::runtime::Handle, dir: String) {
    if SCANNING.swap(true, Ordering::SeqCst) {
        // 正在扫描：记录待扫目录，本轮结束后自动补扫
        *PENDING_DIR.lock().unwrap() = Some(dir);
        return;
    }
    let Some(u) = ui.upgrade() else {
        SCANNING.store(false, Ordering::SeqCst);
        return;
    };
    u.set_local_scan_dir(dir.clone().into());
    u.set_local_scanning(true);

    let ui_task = ui.clone();
    let rt_task = rt.clone();
    rt.spawn_blocking(move || {
        let res = crate::network::netease::local::scan_local_resources(&dir, true);
        let _ = slint::invoke_from_event_loop(move || {
            SCANNING.store(false, Ordering::SeqCst);
            if let Some(u) = ui_task.upgrade() {
                u.set_local_scanning(false);
                match res {
                    Ok((songs, videos)) => {
                        let entries: Vec<Entry> = songs
                            .iter()
                            .map(|v| Entry::from_json(v, Kind::Song))
                            .chain(videos.iter().map(|v| Entry::from_json(v, Kind::Video)))
                            .collect();
                        log::info!(
                            target: "local",
                            "扫描完成: {} 首歌曲 / {} 个视频（{}）",
                            songs.len(),
                            videos.len(),
                            dir
                        );
                        *SCANNED_DIR.lock().unwrap() = dir;
                        set_stats(&u, &entries);
                        *ENTRIES.lock().unwrap() = entries;
                        apply_filter(&u);
                    }
                    Err(e) => {
                        log::warn!(target: "local", "扫描本地资源失败: {e}");
                    }
                }
            }
            // 扫描期间又选了新目录：立即补扫（保证最终展示用户最后选择的目录）
            if let Some(next) = PENDING_DIR.lock().unwrap().take() {
                start_scan(ui_task, rt_task, next);
            }
        });
    });
}

pub fn setup(ctx: &Ctx) {
    // 页面切入：9=本地资源。同目录已扫描过则直接复用结果，避免每次切页都全盘扫描。
    let ui_pf = ctx.ui.clone();
    let rt_pf = ctx.rt.clone();
    let shared_pf = ctx.shared.clone();
    super::on_page_fill(ctx, move |nav| {
        if nav != 9 {
            return;
        }
        let Some(u) = ui_pf.upgrade() else { return };
        let dir = crate::persist::config::local_dir(&shared_pf.borrow())
            .to_string_lossy()
            .to_string();
        u.set_local_scan_dir(dir.clone().into());
        let scanned = SCANNED_DIR.lock().unwrap().clone();
        if scanned != dir {
            start_scan(ui_pf.clone(), rt_pf.clone(), dir);
        }
    });

    // 搜索文本变化：实时过滤（Entry 已预存小写检索串，纯内存过滤）
    let ui_sh = ctx.ui.clone();
    ctx.app.clone_strong().on_local_search_changed(move |text| {
        if let Some(u) = ui_sh.upgrade() {
            u.set_local_search_text(text);
            apply_filter(&u);
        }
    });

    // 分类切换：数据已预填，无需 Rust 侧操作
    ctx.app.clone_strong().on_local_tab_changed(move |idx| {
        log::info!(target: "local", "tab changed to {idx}");
    });

    // 选择扫描目录：原生文件夹对话框 → 写配置（立即落盘）→ 重新扫描
    let ui_pd = ctx.ui.clone();
    let shared_pd = ctx.shared.clone();
    let rt_pd = ctx.rt.clone();
    ctx.app.clone_strong().on_local_pick_dir(move || {
        if ui_pd.upgrade().is_none() {
            return;
        }
        let current = crate::persist::config::local_dir(&shared_pd.borrow())
            .to_string_lossy()
            .to_string();
        let Some(picked) = tinyfiledialogs::select_folder_dialog(&crate::i18n::t("dialog-pick-local-dir"), &current) else {
            return;
        };
        {
            let mut cfg = shared_pd.borrow_mut();
            cfg.local.dir = picked.clone();
            cfg.save();
        }
        log::info!(target: "local", "扫描目录 → {picked}");
        start_scan(ui_pd.clone(), rt_pd.clone(), picked);
    });

    // 重新扫描当前目录（不改变配置）
    let ui_rs = ctx.ui.clone();
    let shared_rs = ctx.shared.clone();
    let rt_rs = ctx.rt.clone();
    ctx.app.clone_strong().on_local_rescan(move || {
        if ui_rs.upgrade().is_none() {
            return;
        }
        let dir = crate::persist::config::local_dir(&shared_rs.borrow())
            .to_string_lossy()
            .to_string();
        start_scan(ui_rs.clone(), rt_rs.clone(), dir);
    });

    // 播放本地视频：用 MV 播放器播放本地文件（file:// 直连）
    let ui_pv = ctx.ui.clone();
    let client_pv = ctx.client.clone();
    let rt_pv = ctx.rt.clone();
    ctx.app.clone_strong().on_local_play_video(move |id| {
        if ui_pv.upgrade().is_none() {
            return;
        }
        let (path, name) = {
            let entries = ENTRIES.lock().unwrap();
            let Some(e) = entries.get(id as usize) else { return };
            if e.kind != Kind::Video {
                return;
            }
            (e.path.clone(), e.name.clone())
        };
        if path.is_empty() {
            return;
        }
        // 复用 MV 播放器播放本地视频：MvPlayer.start_playback 接受本地路径
        // （非 http(s) 不走代理，ffmpeg 直读文件）。
        crate::handlers::mv::player::open_local_video(&ui_pv, &client_pv, &rt_pv, path, name);
    });
}
