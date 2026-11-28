//! 移动端回调层（与桌面 `src/handlers` 独立）：首页点歌、播放控制、进度轮询。
//!
//! 队列保存在 `MobileCtx`，与 AppWindow 的 `queue` 模型保持同步；
//! 音频解码在 tokio 运行时后台执行，播放门面走 `crate::platform::audio`。
//! 所有回调均在 UI 线程触发（Slint 回调语义），因此可直接读写 UI 属性。

use crate::network::netease::client::NeteaseClient;
use crate::persist::cache;
use crate::persist::config::AppConfig;
use crate::platform::audio as player;
use slint::{ComponentHandle, Model};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 播放失败标志：async 取流任务置位，UI 进度轮询消费（提示/标记坏歌/自动续播）。
static PLAY_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 移动端 UI 回调共享上下文。
pub struct MobileCtx {
    pub ui: slint::Weak<crate::AppWindow>,
    pub rt: tokio::runtime::Handle,
    pub client: Arc<NeteaseClient>,
    /// 播放队列（与 UI queue 模型同步；首页数据到达后首次点歌/切歌时刷新）
    pub queue: RefCell<Vec<crate::Song>>,
    /// 当前曲目在队列中的索引
    pub cur: Cell<usize>,
    /// 播放模式 0=列表循环 1=单曲循环 2=顺序循环
    pub mode: Cell<i32>,
    /// 搜索请求序号：每次发起搜索自增。响应回到 UI 线程后先比对序号，过期响应直接丢弃。
    /// 用 `Arc<AtomicU64>` 而非 `Cell`：回投到事件循环的闭包必须 Send，不能捕获 Rc/Cell。
    pub search_seq: Arc<AtomicU64>,
    /// 最近一次有效搜索关键词（切换类型时复用）
    pub search_kw: RefCell<String>,
    /// 详情请求序号：打开/关闭详情时自增，过期响应直接丢弃（防快速切换歌单串台）。
    pub detail_seq: Arc<AtomicU64>,
    /// 分类浏览请求序号：打开页面/切换分类时自增，过期响应与补图回调直接丢弃。
    pub browse_seq: Arc<AtomicU64>,
    /// 歌单/专辑下一页偏移；跨后台响应回写，故使用原子量。
    pub browse_offset: Arc<AtomicU64>,
    /// 播放请求序号：每次点歌自增；旧请求晚到的音频字节直接丢弃，避免覆盖后点的歌。
    pub play_seq: Arc<AtomicU64>,
    /// 当前曲目序号：仅在切歌时自增；歌词请求按它校验（seek 不影响歌词）。
    pub song_seq: Arc<AtomicU64>,
    /// 用户配置快照（设置页修改后写盘；与桌面共用同一份 config.toml）
    pub cfg: RefCell<AppConfig>,
    /// 封面主色缓存（按 song.id）：避免每次切歌重复解码采样
    pub cover_colors: RefCell<HashMap<i32, slint::Color>>,
    /// 队列快照上次落盘时间（epoch 秒）：定时保存进度时判断间隔
    pub last_qsave: Cell<u64>,
    /// 本次播放是否由"自动切歌"发起（播完/跳过）：失败时据此决定是否继续切下一首
    pub auto_next: Cell<bool>,
    /// 自动切歌连续跳过次数（防全部坏歌时死循环）
    pub auto_skip: Cell<usize>,
}

/// 缓存占用字节 → 人类可读文本（KB / MB / GB）。
fn format_usage(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes < MB {
        format!("{} KB", bytes / KB)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    }
}

/// 设置页缓存上限档位 → MB（0=不限）
fn cache_limit_mb(idx: i32) -> u64 {
    match idx {
        1 => 512,
        2 => 1024,
        3 => 2048,
        _ => 0,
    }
}

/// 配置中的上限 MB → 档位（用于设置页回显）
fn cache_limit_index(mb: u64) -> i32 {
    match mb {
        0 => 0,
        v if v <= 512 => 1,
        v if v <= 1024 => 2,
        _ => 3,
    }
}

/// 语义化版本比较：latest 是否比 current 新（忽略 v 前缀与预发布后缀）。
fn version_is_newer(latest: &str, current: &str) -> bool {
    fn parse(v: &str) -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|s| {
                s.chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
            })
            .map(|s| s.parse().unwrap_or(0))
            .collect()
    }
    parse(latest) > parse(current)
}

/// 更新下载页（Release 列表最新版）
const UPDATE_PAGE: &str = "https://github.com/hlshell/zplayer_music/releases/latest";

/// 查询 GitHub 最新 Release；有新版返回 (版本号, 更新说明)。
/// 与桌面端 network::update 使用同一仓库与 UA，移动端仅做检查 + 引导下载（APK 安装由系统完成）。
async fn check_update_latest() -> Result<Option<(String, String)>, String> {
    let client = crate::network::proxy::build_client(Some(std::time::Duration::from_secs(15)));
    let resp = client
        .get("https://api.github.com/repos/hlshell/zplayer_music/releases/latest")
        .header("User-Agent", concat!("zplayer-music/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回 {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| format!("解析响应失败: {e}"))?;
    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or_default();
    if tag.is_empty() {
        return Err("未找到最新版本".into());
    }
    let latest = tag.trim_start_matches('v').to_string();
    if !version_is_newer(&latest, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok(Some((latest, notes)))
}

/// 读取任意 Song 模型为 Vec（仅 UI 线程使用）。
fn read_song_model(m: &slint::ModelRc<crate::Song>) -> Vec<crate::Song> {
    let mut out = Vec::with_capacity(m.row_count());
    for i in 0..m.row_count() {
        if let Some(s) = m.row_data(i) {
            out.push(s);
        }
    }
    out
}

/// 读取 UI 侧 queue 模型为 Vec（仅 UI 线程使用）。
fn read_ui_queue(ui: &crate::AppWindow) -> Vec<crate::Song> {
    read_song_model(&ui.get_queue())
}

/// 取当前队列快照：UI 队列非空时以其为准并回写 ctx；否则沿用 ctx 内队列
/// （启动早期首页数据尚未到达，UI 队列仍为空）。
fn queue_snapshot(ctx: &MobileCtx, ui: &crate::AppWindow) -> Vec<crate::Song> {
    let ui_q = read_ui_queue(ui);
    if ui_q.is_empty() {
        return ctx.queue.borrow().clone();
    }
    *ctx.queue.borrow_mut() = ui_q.clone();
    ui_q
}

/// 从首页数据池解析歌曲元数据（Hero 精选池 → 新歌速递池）。
fn resolve_from_pools(id: i32) -> Option<crate::Song> {
    let hero = {
        let pool = crate::load::HERO_POOL.lock().ok()?;
        pool.iter().find(|s| s.id == id).cloned()
    };
    if let Some(b) = hero {
        return Some(song_from_base(&b));
    }
    let ns = {
        let pool = crate::load::NEW_SONG_POOL.lock().ok()?;
        pool.iter().find(|s| s.id == id).cloned()
    };
    ns.as_ref().map(song_from_base)
}

/// 在 UI 模型里按 id 找歌（搜索结果/详情列表等非队列来源）。
fn find_in_model(m: &slint::ModelRc<crate::Song>, id: i32) -> Option<crate::Song> {
    for i in 0..m.row_count() {
        if let Some(s) = m.row_data(i) {
            if s.id == id {
                return Some(s);
            }
        }
    }
    None
}

/// 在 UI 各列表模型与数据池里按 id 找歌（操作抽屉展示与切歌解析共用）。
fn find_song_anywhere(ui: &crate::AppWindow, id: i32) -> Option<crate::Song> {
    for m in [
        ui.get_queue(),
        ui.get_detail_songs(),
        ui.get_search_songs(),
        ui.get_recent_songs(),
        ui.get_liked_songs(),
    ] {
        if let Some(s) = find_in_model(&m, id) {
            return Some(s);
        }
    }
    resolve_from_pools(id)
}

fn song_from_base(b: &crate::load::SongBase) -> crate::Song {
    let cover_url = b.url.clone();
    crate::Song {
        id: b.id,
        name: b.name.clone(),
        artist: b.artist.clone(),
        album: b.album.clone(),
        cover: crate::load::cover_now(&cover_url),
        duration: b.duration.clone(),
        duration_sec: b.duration_sec,
        url: cover_url.into(),
        liked: crate::persist::liked::is_liked(b.id as u32 as i64),
        bad: is_bad_song(b.id),
    }
}

/// 取色失败时的回退主色（顺序与设置页主题色一致）。
const FALLBACK_COVER_COLORS: [u32; 4] = [0x6c5ce7, 0x0ea371, 0xea580c, 0x3b82f6];

/// 封面主色：按 song.id 缓存；无图/解码失败回退当前主题色，保证播放层背景始终有色。
fn cover_color_of(_ui: &crate::AppWindow, ctx: &MobileCtx, song: &crate::Song) -> slint::Color {
    if let Some(c) = ctx.cover_colors.borrow().get(&song.id).copied() {
        return c;
    }
    let color = cover_color_from_image(&crate::load::cover_now(&song.url)).unwrap_or_else(|| {
        // 主题色切换功能已废除，封面取色失败时固定回退到默认（紫青）主色
        let idx = 0usize;
        let rgb = FALLBACK_COVER_COLORS[idx];
        slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    });
    ctx.cover_colors.borrow_mut().insert(song.id, color);
    color
}

/// 8 像素步长采样平均：跳过半透明与近黑/近白像素，过暗时按比例提亮到 96 以上。
fn cover_color_from_image(img: &slint::Image) -> Option<slint::Color> {
    if img.size().width == 0 {
        return None;
    }
    let buf = img.to_rgba8()?;
    let (w, h) = (buf.width() as usize, buf.height() as usize);
    let step = 8usize;
    let (mut n, mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64, 0u64);
    let mut y = 0usize;
    while y < h {
        let mut x = 0usize;
        while x < w {
            let p = buf.as_slice()[y * w + x];
            if p.a >= 128 {
                let (r, g, b) = (p.r as u32, p.g as u32, p.b as u32);
                if !(r < 18 && g < 18 && b < 18) && !(r > 240 && g > 240 && b > 240) {
                    n += 1;
                    sr += r as u64;
                    sg += g as u64;
                    sb += b as u64;
                }
            }
            x += step;
        }
        y += step;
    }
    if n == 0 {
        return None;
    }
    let (mut r, mut g, mut b) = ((sr / n) as f32, (sg / n) as f32, (sb / n) as f32);
    let peak = r.max(g).max(b);
    if peak < 64.0 && peak > 0.0 {
        let k = 96.0 / peak;
        r = (r * k).min(255.0);
        g = (g * k).min(255.0);
        b = (b * k).min(255.0);
    }
    Some(slint::Color::from_rgb_u8(r as u8, g as u8, b as u8))
}

/// 业务码校验：响应顶层 code 非 200 视为失败。
fn biz_ok(v: &serde_json::Value) -> bool {
    v.get("code").and_then(|c| c.as_i64()) == Some(200)
}

/// 详情响应中间态：标题 / 描述 / 封面 URL / 歌曲（全部 Send）。
type DetailData = (String, String, String, Vec<crate::load::SongBase>);

/// 歌单详情：tracks 不足时用 trackIds + song_detail 兜底补全（上限 500 首）。
async fn fetch_playlist_detail(client: &NeteaseClient, uid: u64) -> Result<DetailData, String> {
    let v = crate::network::service::playlist_detail(client, uid, 1000).await?;
    if !biz_ok(&v) {
        return Err("歌单加载失败".to_string());
    }
    let p = v
        .get("playlist")
        .filter(|x| !x.is_null())
        .ok_or_else(|| "歌单不存在".to_string())?;
    let title = p.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let desc: String = p
        .get("description")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let cover = p.get("coverImgUrl").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let mut tracks = p.get("tracks").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let total_ids: Vec<i64> = p
        .get("trackIds")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("id").and_then(|i| i.as_i64()))
                .take(500)
                .collect()
        })
        .unwrap_or_default();
    if tracks.len() < total_ids.len() && !total_ids.is_empty() {
        if let Ok(dv) = crate::network::service::song_detail(client, &total_ids).await {
            if biz_ok(&dv) {
                let full = dv.get("songs").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                if !full.is_empty() {
                    tracks = full;
                }
            }
        }
    }
    Ok((title, desc, cover, crate::load::parse_tracks(&tracks)))
}

/// 专辑详情：album 元信息 + songs。
async fn fetch_album_detail(client: &NeteaseClient, uid: u64) -> Result<DetailData, String> {
    let v = crate::network::service::album_songs(client, uid).await?;
    if !biz_ok(&v) {
        return Err("专辑加载失败".to_string());
    }
    let al = v
        .get("album")
        .filter(|x| !x.is_null())
        .ok_or_else(|| "专辑不存在".to_string())?;
    let title = al.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let desc = al
        .get("artist")
        .and_then(|x| x.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let cover = al.get("picUrl").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let songs = v.get("songs").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    Ok((title, desc, cover, crate::load::parse_tracks(&songs)))
}

/// 打开歌单/专辑/电台详情：序号自增 → 置加载态 → 拉取/读取节目池 → UI 线程按序号防串回填。
fn open_detail(ctx: &Rc<MobileCtx>, id: i32, kind: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let seq = ctx.detail_seq.fetch_add(1, Ordering::SeqCst) + 1;

    ui.set_detail_open(true);
    ui.set_detail_kind(kind);
    ui.set_detail_title("".into());
    ui.set_detail_desc("".into());
    ui.set_detail_cover(slint::Image::default());
    ui.set_detail_songs(crate::load::model(Vec::new()));
    ui.set_detail_loading(true);
    ui.set_detail_error("".into());

    // Slint int 是 i32：优先从 ID 桥接表还原 64 位资源 ID，未登记时按 u32 兼容旧路径。
    let uid = crate::core::ids::resolve(kind, id) as u64;

    // 每日推荐：详情直接读取首页 HERO_POOL 精选池，无需再发网络请求。
    if kind == 4 {
        let rows: Vec<crate::Song> = {
            let pool = crate::load::HERO_POOL.lock().unwrap();
            pool.iter().map(song_from_base).collect()
        };
        let count = rows.len();
        // 首页仅缓存过 hero-cover 一张，其余歌曲封面需批量补齐（自带并发闸 + 同 URL 合并）。
        let urls: Vec<(String, cache::CoverType)> = rows
            .iter()
            .filter(|r| !r.url.is_empty() && crate::load::cover_now(r.url.as_str()).size().width == 0)
            .map(|r| (r.url.to_string(), cache::CoverType::Song))
            .collect();
        ui.set_detail_title("每日推荐".into());
        ui.set_detail_desc(format!("共 {count} 首 · 为你精选").as_str().into());
        ui.set_detail_cover(ui.get_hero_cover());
        ui.set_detail_songs(crate::load::model(rows));
        ui.set_detail_loading(false);
        if !urls.is_empty() {
            let ui_weak = ctx.ui.clone();
            let seq_now = ctx.detail_seq.clone();
            ctx.rt.spawn(async move {
                // 批量拉取并写入封面缓存，随后回 UI 线程按 URL 重新取图回填。
                let _ = crate::load::fetch_covers(&urls).await;
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(u) = ui_weak.upgrade() else { return; };
                    if seq_now.load(Ordering::SeqCst) != seq { return; }
                    let m = u.get_detail_songs();
                    for i in 0..m.row_count() {
                        if let Some(mut row) = m.row_data(i) {
                            let img = crate::load::cover_now(row.url.as_str());
                            if img.size().width > 0 {
                                row.cover = img;
                                m.set_row_data(i, row);
                            }
                        }
                    }
                });
            });
        }
        return;
    }

    // 电台接口仅返回推荐节目：详情直接读取按电台分组的 mainSong，无需再发网络请求。
    if kind == 3 {
        let data = {
            let pool = RADIO_POOL.lock().unwrap();
            pool.iter().find(|(rid, _)| *rid == uid as i64).map(|(_, programs)| {
                let radio = programs.first().and_then(|p| p.get("radio"));
                let title = radio
                    .and_then(|r| r.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let desc = radio
                    .and_then(|r| r.get("desc"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cover = norm_cover(
                    // 电台 picUrl 常为空；空时回退到节目封面 coverUrl。
                    radio.and_then(|r| r.get("picUrl")).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                        .or_else(|| programs.first().and_then(|p| p.get("coverUrl")).and_then(|v| v.as_str()))
                        .unwrap_or(""),
                );
                let tracks: Vec<serde_json::Value> = programs
                    .iter()
                    .filter_map(|p| p.get("mainSong").cloned())
                    .collect();
                (title, desc, cover, crate::load::parse_tracks(&tracks))
            })
        };
        match data {
            Some((title, desc, cover_url, songs)) => {
                ui.set_detail_title(title.as_str().into());
                ui.set_detail_desc(desc.as_str().into());
                let img = crate::load::cover_now(&cover_url);
                ui.set_detail_cover(img);
                let rows: Vec<crate::Song> = songs.iter().map(song_from_base).collect();
                ui.set_detail_songs(crate::load::model(rows));
                if !cover_url.is_empty() {
                    let w = ctx.ui.clone();
                    let seq_now = ctx.detail_seq.clone();
                    crate::load::cover_warm(&cover_url, cache::CoverType::Radio, move |img| {
                        if seq_now.load(Ordering::SeqCst) != seq || img.size().width == 0 {
                            return;
                        }
                        if let Some(u) = w.upgrade() {
                            u.set_detail_cover(img);
                        }
                    });
                }
            }
            None => ui.set_detail_error("电台不存在或已下架".into()),
        }
        ui.set_detail_loading(false);
        return;
    }

    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    let seq_now = ctx.detail_seq.clone();
    ctx.rt.spawn(async move {
        let got = if kind == 0 {
            fetch_playlist_detail(&client, uid).await
        } else {
            fetch_album_detail(&client, uid).await
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            // 序号防串：期间已打开/关闭其他详情则丢弃本次响应
            if seq_now.load(Ordering::SeqCst) != seq {
                return;
            }
            match got {
                Ok((title, desc, cover_url, songs)) => {
                    u.set_detail_title(title.as_str().into());
                    u.set_detail_desc(desc.as_str().into());
                    let img = crate::load::cover_now(&cover_url);
                    if img.size().width > 0 {
                        u.set_detail_cover(img);
                    } else if !cover_url.is_empty() {
                        let w = u.as_weak();
                        let seq_cover = seq_now.clone();
                        crate::load::cover_warm(&cover_url, browse_cover_type(kind), move |img| {
                            if img.size().width == 0 || seq_cover.load(Ordering::SeqCst) != seq {
                                return;
                            }
                            if let Some(u) = w.upgrade() {
                                u.set_detail_cover(img);
                            }
                        });
                    }
                    let rows: Vec<crate::Song> = songs.iter().map(song_from_base).collect();

                    // 批量补齐歌曲封面（歌单/专辑详情歌曲数量多，部分封面初始为空）
                    let urls: Vec<(String, cache::CoverType)> = rows
                        .iter()
                        .filter(|r| !r.url.is_empty() && crate::load::cover_now(r.url.as_str()).size().width == 0)
                        .map(|r| (r.url.to_string(), cache::CoverType::Song))
                        .collect();

                    u.set_detail_songs(crate::load::model(rows));
                    u.set_detail_error("".into());

                    if !urls.is_empty() {
                        let ui_w = u.as_weak();
                        let seq_f = seq_now.clone();
                        if let Some(rt) = crate::load::runtime() {
                            rt.spawn(async move {
                                let _ = crate::load::fetch_covers(&urls).await;
                                let _ = slint::invoke_from_event_loop(move || {
                                    let Some(uu) = ui_w.upgrade() else { return; };
                                    if seq_f.load(Ordering::SeqCst) != seq { return; }
                                    let m = uu.get_detail_songs();
                                    for i in 0..m.row_count() {
                                        if let Some(mut row) = m.row_data(i) {
                                            let img = crate::load::cover_now(row.url.as_str());
                                            if img.size().width > 0 {
                                                row.cover = img;
                                                m.set_row_data(i, row);
                                            }
                                        }
                                    }
                                });
                            });
                        }
                    }
                }
                Err(e) => {
                    log::warn!("[mobile] 详情加载失败: kind={kind} id={id} err={e}");
                    u.set_detail_error("加载失败，请重试".into());
                }
            }
            u.set_detail_loading(false);
        });
    });
}

// ============================================================================
// 分类浏览：歌单 / 专辑 / 排行榜 / 电台
// ============================================================================

const BROWSE_LIMIT: u32 = 30;

/// 跨后台任务传递的浏览条目（slint::Image 只在 UI 线程构造）。
#[derive(Clone)]
struct BrowseRow {
    id: i32,
    name: String,
    sub: String,
    url: String,
    cat: String,
}

struct BrowseData {
    rows: Vec<BrowseRow>,
    cats: Vec<String>,
    has_more: bool,
    /// 仅电台使用；通过序号校验后再写入静态池，避免旧请求覆盖新请求。
    radio_programs: Vec<(i64, Vec<serde_json::Value>)>,
}

/// 推荐节目按真实电台 ID 分组；详情页从 program.mainSong 构建歌曲列表。
static RADIO_POOL: Mutex<Vec<(i64, Vec<serde_json::Value>)>> = Mutex::new(Vec::new());
/// 电台全量去重条目，分类切换在客户端过滤，不重复请求。
static RADIO_ROWS: Mutex<Vec<BrowseRow>> = Mutex::new(Vec::new());

fn browse_cover_type(kind: i32) -> cache::CoverType {
    match kind {
        1 => cache::CoverType::Album,
        3 => cache::CoverType::Radio,
        4 => cache::CoverType::Mv,
        _ => cache::CoverType::Playlist,
    }
}

/// 网易云部分封面仍是 http 地址，Android 默认禁止明文流量，统一升级为 https。
fn norm_cover(url: &str) -> String {
    url.replacen("http://", "https://", 1)
}

fn browse_item(row: &BrowseRow) -> crate::BrowseItem {
    crate::BrowseItem {
        id: row.id,
        name: row.name.as_str().into(),
        sub: row.sub.as_str().into(),
        cover: crate::load::cover_now(&row.url),
        url: row.url.as_str().into(),
    }
}

/// 回填浏览模型并按 ID 补图；请求序号与页面类型双重校验，杜绝切页后旧图串行。
fn publish_browse(
    ui: &crate::AppWindow,
    rows: &[BrowseRow],
    append: bool,
    kind: i32,
    seq: u64,
    seq_now: Arc<AtomicU64>,
) {
    let old = ui.get_browse_items();
    let mut all: Vec<crate::BrowseItem> = if append {
        (0..old.row_count()).filter_map(|i| old.row_data(i)).collect()
    } else {
        Vec::new()
    };
    all.extend(rows.iter().map(browse_item));
    ui.set_browse_items(crate::load::model(all));

    // 批量补图：fetch_covers 自带并发闸与同 URL 合并，避免几十个请求同时打出去
    // 导致部分封面超时缺失（这也是此前偶发空封面的根因）。
    let urls: Vec<(String, cache::CoverType)> = rows
        .iter()
        .filter(|r| !r.url.is_empty() && crate::load::cover_now(&r.url).size().width == 0)
        .map(|r| (r.url.clone(), browse_cover_type(kind)))
        .collect();
    if urls.is_empty() {
        return;
    }
    let Some(rt) = crate::load::runtime() else { return; };
    let w = ui.as_weak();
    rt.spawn(async move {
        let covers = crate::load::fetch_covers(&urls).await;
        if covers.is_empty() {
            return;
        }
        let _ = slint::invoke_from_event_loop(move || {
            if seq_now.load(Ordering::SeqCst) != seq {
                return;
            }
            let Some(u) = w.upgrade() else { return; };
            if u.get_browse_kind() != kind {
                return;
            }
            let m = u.get_browse_items();
            for i in 0..m.row_count() {
                let Some(mut item) = m.row_data(i) else { continue; };
                if item.cover.size().width > 0 {
                    continue;
                }
                if let Some(Some(buf)) = covers.get(item.url.as_str()) {
                    item.cover = slint::Image::from_rgba8(buf.clone());
                    m.set_row_data(i, item);
                }
            }
        });
    });
}

/// 拉取一个浏览页；电台响应同时建立全量条目池和节目池。
async fn fetch_browse(
    client: &NeteaseClient,
    kind: i32,
    cat: &str,
    offset: u32,
) -> Result<BrowseData, String> {
    match kind {
        0 => {
            let v = crate::network::service::top_playlists_by_cat(client, cat, BROWSE_LIMIT, offset).await?;
            if !biz_ok(&v) {
                return Err("歌单加载失败".into());
            }
            let arr = v.get("playlists").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let has_more = v.get("more").and_then(|x| x.as_bool())
                .unwrap_or(arr.len() == BROWSE_LIMIT as usize);
            let rows = arr.iter().filter_map(|p| {
                let full = p.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                if full == 0 {
                    return None;
                }
                let plays = crate::core::song_util::fmt_plays_cat(
                    p.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0),
                );
                Some(BrowseRow {
                    id: crate::core::ids::remember(0, full),
                    name: p.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sub: format!("{plays} 次播放"),
                    url: norm_cover(p.get("coverImgUrl").and_then(|x| x.as_str()).unwrap_or("")),
                    cat: String::new(),
                })
            }).collect();
            Ok(BrowseData { rows, cats: Vec::new(), has_more, radio_programs: Vec::new() })
        }
        1 => {
            let v = crate::network::service::new_albums(client, cat, BROWSE_LIMIT, offset).await?;
            if !biz_ok(&v) {
                return Err("专辑加载失败".into());
            }
            let arr = v.get("albums").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let has_more = v.get("more").and_then(|x| x.as_bool())
                .unwrap_or(arr.len() == BROWSE_LIMIT as usize);
            let rows = arr.iter().filter_map(|a| {
                let full = a.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                if full == 0 {
                    return None;
                }
                Some(BrowseRow {
                    id: crate::core::ids::remember(1, full),
                    name: a.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sub: a.get("artist").and_then(|x| x.get("name"))
                        .and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    url: norm_cover(a.get("picUrl").and_then(|x| x.as_str()).unwrap_or("")),
                    cat: String::new(),
                })
            }).collect();
            Ok(BrowseData { rows, cats: Vec::new(), has_more, radio_programs: Vec::new() })
        }
        2 => {
            let v = crate::network::service::toplist(client).await?;
            if !biz_ok(&v) {
                return Err("排行榜加载失败".into());
            }
            let arr = v.get("list").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let rows = arr.iter().filter_map(|t| {
                let full = t.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                if full == 0 {
                    return None;
                }
                Some(BrowseRow {
                    // 榜单详情与歌单详情同构，直接复用 playlist_detail。
                    id: crate::core::ids::remember(0, full),
                    name: t.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sub: t.get("updateFrequency").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    url: norm_cover(t.get("coverImgUrl").and_then(|x| x.as_str()).unwrap_or("")),
                    cat: String::new(),
                })
            }).collect();
            Ok(BrowseData { rows, cats: Vec::new(), has_more: false, radio_programs: Vec::new() })
        }
        3 => {
            let v = crate::network::service::dj_personalized_recommend(client, 80).await?;
            if !biz_ok(&v) {
                return Err("电台加载失败".into());
            }
            // 与桌面端实测接口一致：result[] 每项为 { program: { radio, mainSong, ... } }。
            let arr = v.get("result").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let mut program_map: HashMap<i64, Vec<serde_json::Value>> = HashMap::new();
            for it in &arr {
                if let Some(prog) = it.get("program") {
                    if let Some(rid) = prog.get("radio").and_then(|r| r.get("id")).and_then(|x| x.as_i64()) {
                        program_map.entry(rid).or_default().push(prog.clone());
                    }
                }
            }

            let mut rows: Vec<BrowseRow> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut cat_count: HashMap<String, u32> = HashMap::new();
            for it in &arr {
                let Some(prog) = it.get("program") else { continue; };
                let Some(radio) = prog.get("radio") else { continue; };
                let full = radio.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                if full == 0 || !seen.insert(full) {
                    continue;
                }
                let cat = radio.get("category").and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty()).unwrap_or("其他").to_string();
                let desc = radio.get("desc").and_then(|x| x.as_str()).unwrap_or("");
                rows.push(BrowseRow {
                    id: crate::core::ids::remember(3, full),
                    name: radio.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sub: if desc.is_empty() { cat.clone() } else { desc.to_string() },
                    url: norm_cover(
                        // 电台 picUrl 常为空；空时回退到 result 项顶层的节目封面 picUrl。
                        radio.get("picUrl").and_then(|x| x.as_str()).filter(|s| !s.is_empty())
                            .or_else(|| it.get("picUrl").and_then(|x| x.as_str()))
                            .unwrap_or(""),
                    ),
                    cat: cat.clone(),
                });
                *cat_count.entry(cat).or_insert(0) += 1;
            }
            let mut names: Vec<String> = cat_count.keys().cloned().collect();
            names.sort_by_key(|c| std::cmp::Reverse(cat_count[c]));
            let mut cats = vec!["全部".to_string()];
            cats.extend(names);
            Ok(BrowseData {
                rows,
                cats,
                has_more: false,
                radio_programs: program_map.into_iter().collect(),
            })
        }
        4 => {
            // MV：0=全部MV（mv_all），1=精选MV（mv_exclusive），响应字段一致
            let v = if cat == "精选" {
                crate::network::service::mv_exclusive(client, BROWSE_LIMIT, offset).await?
            } else {
                crate::network::service::mv_all(client, "", BROWSE_LIMIT, offset).await?
            };
            if !biz_ok(&v) {
                return Err("MV 加载失败".into());
            }
            let arr = v.get("data").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let has_more = arr.len() >= BROWSE_LIMIT as usize;
            let rows = arr.iter().filter_map(|m| {
                let full = m.get("id").and_then(|x| x.as_i64()).unwrap_or(0);
                if full == 0 {
                    return None;
                }
                let artist = m.get("artistName").and_then(|x| x.as_str()).unwrap_or("");
                let plays = crate::core::song_util::fmt_plays_cat(
                    m.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0),
                );
                let sub = if artist.is_empty() {
                    format!("{plays} 次播放")
                } else {
                    format!("{artist} · {plays} 次播放")
                };
                Some(BrowseRow {
                    id: crate::core::ids::remember(4, full),
                    name: m.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sub,
                    url: norm_cover(
                        m.get("cover").and_then(|x| x.as_str())
                            .or_else(|| m.get("imgurl").and_then(|x| x.as_str()))
                            .unwrap_or(""),
                    ),
                    cat: String::new(),
                })
            }).collect();
            Ok(BrowseData { rows, cats: Vec::new(), has_more, radio_programs: Vec::new() })
        }
        _ => Err("未知浏览类型".into()),
    }
}

/// 发起当前浏览页请求；append=false 为首屏/分类切换，true 为加载更多。
fn load_browse(ctx: &Rc<MobileCtx>, append: bool) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let kind = ui.get_browse_kind();
    let active = ui.get_browse_active_cat();
    let offset = if append {
        ctx.browse_offset.load(Ordering::SeqCst) as u32
    } else {
        0
    };
    let cat = match kind {
        0 => ui.get_browse_cats().row_data(active as usize)
            .map(|s| s.to_string()).unwrap_or_else(|| "全部".to_string()),
        1 => ["ALL", "ZH", "EU", "KR", "JP"]
            .get(active.max(0) as usize).unwrap_or(&"ALL").to_string(),
        4 => if active == 1 { "精选".to_string() } else { "全部".to_string() },
        _ => String::new(),
    };
    let seq = ctx.browse_seq.load(Ordering::SeqCst);
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    let seq_now = ctx.browse_seq.clone();
    let offset_now = ctx.browse_offset.clone();
    ctx.rt.spawn(async move {
        let got = fetch_browse(&client, kind, &cat, offset).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            if seq_now.load(Ordering::SeqCst) != seq {
                return;
            }
            match got {
                Ok(data) => {
                    let BrowseData { rows, cats, has_more, radio_programs } = data;
                    if kind == 3 {
                        *RADIO_POOL.lock().unwrap() = radio_programs;
                        *RADIO_ROWS.lock().unwrap() = rows.clone();
                        let cats: Vec<slint::SharedString> = cats.iter()
                            .map(|s| s.as_str().into()).collect();
                        u.set_browse_cats(crate::load::model(cats));
                    }
                    publish_browse(&u, &rows, append, kind, seq, seq_now.clone());
                    offset_now.store(offset as u64 + BROWSE_LIMIT as u64, Ordering::SeqCst);
                    u.set_browse_has_more(has_more);
                    u.set_browse_error("".into());
                }
                Err(e) => {
                    log::warn!("[mobile] 分类浏览加载失败: kind={kind} offset={offset} err={e}");
                    u.set_browse_error("加载失败，请重试".into());
                }
            }
            u.set_browse_loading(false);
        });
    });
}

fn open_browse(ctx: &Rc<MobileCtx>, kind: i32) {
    if !(0..=4).contains(&kind) {
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let (title, cats): (&str, &[&str]) = match kind {
        0 => ("歌单", &["全部", "华语", "流行", "摇滚", "民谣", "电子", "轻音乐", "ACG"]),
        1 => ("专辑", &["全部", "华语", "欧美", "韩国", "日本"]),
        2 => ("排行榜", &[]),
        3 => ("电台", &[]),
        _ => ("MV", &["全部", "精选"]),
    };
    ctx.browse_seq.fetch_add(1, Ordering::SeqCst);
    ctx.browse_offset.store(0, Ordering::SeqCst);
    ui.set_detail_open(false);
    ui.set_current_nav(4 + kind);
    ui.set_browse_kind(kind);
    ui.set_browse_title(title.into());
    ui.set_browse_cats(crate::load::model(
        cats.iter().map(|s| slint::SharedString::from(*s)).collect::<Vec<_>>(),
    ));
    ui.set_browse_active_cat(0);
    ui.set_browse_items(crate::load::model(Vec::new()));
    ui.set_browse_loading(true);
    ui.set_browse_error("".into());
    ui.set_browse_has_more(kind == 0 || kind == 1 || kind == 4);
    if kind == 3 {
        RADIO_ROWS.lock().unwrap().clear();
        RADIO_POOL.lock().unwrap().clear();
    }
    load_browse(ctx, false);
}

fn browse_cat_changed(ctx: &Rc<MobileCtx>, idx: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let kind = ui.get_browse_kind();
    if idx < 0 || idx as usize >= ui.get_browse_cats().row_count() || kind == 2 {
        return;
    }
    ui.set_browse_active_cat(idx);
    ui.set_browse_error("".into());

    // 电台已按分类保存在内存中，切换时仅过滤，不重新拉取。
    if kind == 3 {
        let selected = ui.get_browse_cats().row_data(idx as usize)
            .map(|s| s.to_string()).unwrap_or_default();
        let all = RADIO_ROWS.lock().unwrap().clone();
        let rows: Vec<BrowseRow> = all.into_iter()
            .filter(|r| idx == 0 || r.cat == selected)
            .collect();
        ui.set_browse_loading(false);
        ui.set_browse_has_more(false);
        let seq = ctx.browse_seq.load(Ordering::SeqCst);
        publish_browse(&ui, &rows, false, kind, seq, ctx.browse_seq.clone());
        return;
    }

    ctx.browse_seq.fetch_add(1, Ordering::SeqCst);
    ctx.browse_offset.store(0, Ordering::SeqCst);
    ui.set_browse_items(crate::load::model(Vec::new()));
    ui.set_browse_loading(true);
    ui.set_browse_has_more(true);
    load_browse(ctx, false);
}

fn browse_load_more(ctx: &Rc<MobileCtx>) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let kind = ui.get_browse_kind();
    // 排行榜/电台一次性返回全部，不支持分页；歌单/专辑/MV 支持加载更多
    if kind == 2 || kind == 3 || ui.get_browse_loading() || !ui.get_browse_has_more() {
        return;
    }
    ui.set_browse_loading(true);
    ui.set_browse_error("".into());
    load_browse(ctx, true);
}

/// 首屏加载失败后的重试：复用当前分类与页码起点。
fn browse_retry(ctx: &Rc<MobileCtx>) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    if ui.get_browse_loading() {
        return;
    }
    ui.set_browse_loading(true);
    ui.set_browse_error("".into());
    load_browse(ctx, false);
}


/// 搜索结果中间态（仅含 Send 数据；slint::Image 只能在 UI 线程构造）。
enum SearchHits {
    Songs(Vec<crate::load::SongBase>),
    Playlists(Vec<PlaylistHit>),
    Albums(Vec<AlbumHit>),
}

struct PlaylistHit {
    id: i32,
    name: slint::SharedString,
    plays: slint::SharedString,
    url: String,
}

struct AlbumHit {
    id: i32,
    name: slint::SharedString,
    artist: slint::SharedString,
    year: slint::SharedString,
    url: String,
}

/// publishTime（毫秒时间戳）→ 年份字符串；缺失/非法留空。
fn ms_to_year(ms: i64) -> slint::SharedString {
    if ms <= 0 {
        return slint::SharedString::default();
    }
    // 按实际天数逐年扣减（含闰年），避免按 365.25 估算在年末偏到下一年
    let mut days = ms / 86_400_000;
    let mut year = 1970i64;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let year_days = if leap { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        year += 1;
    }
    year.to_string().into()
}

/// 后台线程解析响应：只产出 Send 结构，不碰 slint::Image。
fn parse_hits(v: &serde_json::Value, ty: i32) -> SearchHits {
    match ty {
        0 => {
            let arr = v
                .pointer("/result/songs")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Songs(crate::load::parse_tracks(&arr))
        }
        1 => {
            let arr = v
                .pointer("/result/playlists")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Playlists(
                arr.iter()
                    .map(|p| PlaylistHit {
                        id: p.get("id").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                        name: p.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                        plays: crate::core::song_util::fmt_plays_cat(
                            p.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0),
                        )
                        .into(),
                        url: p
                            .get("coverImgUrl")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect(),
            )
        }
        _ => {
            let arr = v
                .pointer("/result/albums")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Albums(
                arr.iter()
                    .map(|a| AlbumHit {
                        id: a.get("id").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                        name: a.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                        artist: a
                            .get("artist")
                            .and_then(|x| x.get("name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        year: a
                            .get("publishTime")
                            .and_then(|x| x.as_i64())
                            .map(ms_to_year)
                            .unwrap_or_default(),
                        url: a
                            .get("picUrl")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect(),
            )
        }
    }
}

/// UI 线程落库：封面/渐变在此构造（slint::Image 非 Send）。
fn apply_hits(ui: &crate::AppWindow, hits: SearchHits) {
    match hits {
        SearchHits::Songs(items) => {
            let rows: Vec<crate::Song> = items
                .iter()
                .map(|b| crate::Song {
                    id: b.id,
                    name: b.name.clone(),
                    artist: b.artist.clone(),
                    album: b.album.clone(),
                    cover: crate::load::cover_now(&b.url),
                    duration: b.duration.clone(),
                    duration_sec: b.duration_sec,
                    url: b.url.clone().into(),
                    liked: crate::persist::liked::is_liked(b.id as u32 as i64),
                    bad: is_bad_song(b.id),
                })
                .collect();
            ui.set_search_songs(crate::load::model(rows));
        }
        SearchHits::Playlists(items) => {
            let rows: Vec<crate::Playlist> = items
                .iter()
                .enumerate()
                .map(|(i, p)| crate::Playlist {
                    id: p.id,
                    name: p.name.clone(),
                    plays: p.plays.clone(),
                    cover: crate::load::cover_now(&p.url),
                    from: crate::load::from(i),
                    to: crate::load::to(i),
                })
                .collect();
            ui.set_search_playlists(crate::load::model(rows));
        }
        SearchHits::Albums(items) => {
            let rows: Vec<crate::Album> = items
                .iter()
                .enumerate()
                .map(|(i, a)| crate::Album {
                    id: a.id,
                    name: a.name.clone(),
                    artist: a.artist.clone(),
                    year: a.year.clone(),
                    cover: crate::load::cover_now(&a.url),
                    from: crate::load::from(i),
                    to: crate::load::to(i),
                })
                .collect();
            ui.set_search_albums(crate::load::model(rows));
        }
    }
}

/// 刷新搜索历史模型（搜索页无关键词时的历史列表）。
fn push_search_history(ui: &crate::AppWindow) {
    let words: Vec<slint::SharedString> = crate::persist::history::list()
        .iter()
        .map(|w| w.as_str().into())
        .collect();
    ui.set_search_history(crate::load::model(words));
}

/// 发起一次搜索：序号自增 → 置加载态 → 后台请求 → UI 线程按序号防串回填。
fn start_search(ctx: &Rc<MobileCtx>, keyword: &str, ty: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let kw = keyword.trim().to_string();

    // 空关键词：清空结果并作废在途请求（序号自增让旧响应失效）
    if kw.is_empty() {
        ctx.search_seq.fetch_add(1, Ordering::SeqCst);
        ctx.search_kw.borrow_mut().clear();
        ui.set_search_songs(crate::load::model(Vec::new()));
        ui.set_search_playlists(crate::load::model(Vec::new()));
        ui.set_search_albums(crate::load::model(Vec::new()));
        ui.set_search_loading(false);
        ui.set_search_error("".into());
        ui.set_search_keyword("".into());
        return;
    }

    let seq = ctx.search_seq.fetch_add(1, Ordering::SeqCst) + 1;
    *ctx.search_kw.borrow_mut() = kw.clone();

    ui.set_search_type(ty);
    ui.set_search_keyword(kw.as_str().into());
    ui.set_search_loading(true);
    ui.set_search_error("".into());

    // 记录搜索历史（去重前置）并刷新搜索页历史列表
    crate::persist::history::record(&kw);
    push_search_history(&ui);

    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    let seq_now = ctx.search_seq.clone();
    ctx.rt.spawn(async move {
        // 本地类型 → 网易云搜索类型：0 单曲=1 / 1 歌单=1000 / 2 专辑=10
        let api_ty = match ty {
            0 => 1,
            1 => 1000,
            _ => 10,
        };
        let result = crate::network::service::search(&client, &kw, api_ty, 30, 0).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            // 序号防串：期间已有新搜索发起（或已清空）则丢弃本次响应
            if seq_now.load(Ordering::SeqCst) != seq {
                return;
            }
            match result {
                Ok(v) => {
                    apply_hits(&u, parse_hits(&v, ty));
                    u.set_search_error("".into());
                }
                Err(e) => {
                    log::warn!("[mobile] 搜索失败: keyword={kw} err={e}");
                    u.set_search_error("搜索失败，请重试".into());
                }
            }
            u.set_search_loading(false);
        });
    });
}

/// 刷新最近播放列表：内存记录 → Song 模型；封面命中内存缓存直接填，未命中后台补图。
fn refresh_recent(ui: &crate::AppWindow) {
    let entries = crate::persist::recent::list_by_kind(0);
    let rows: Vec<crate::Song> = entries
        .iter()
        .map(|e| crate::Song {
            id: e.id as u32 as i32,
            name: e.name.as_str().into(),
            artist: e.artist.as_str().into(),
            album: e.album.as_str().into(),
            cover: crate::load::cover_now(&e.cover),
            duration: e.duration.as_str().into(),
            duration_sec: e.duration_sec,
            url: e.cover.as_str().into(),
            liked: crate::persist::liked::is_liked(e.id),
            bad: is_bad_song(e.id as u32 as i32),
        })
        .collect();
    ui.set_recent_songs(crate::load::model(rows));
    // 封面补图：完成后按行 id 回写，不重建整个模型（避免打断滚动位置）
    for e in entries {
        if e.cover.is_empty() {
            continue;
        }
        let id = e.id as u32 as i32;
        let w = ui.as_weak();
        crate::load::cover_warm(&e.cover, cache::CoverType::Song, move |img| {
            let Some(u) = w.upgrade() else { return; };
            let m = u.get_recent_songs();
            for i in 0..m.row_count() {
                if let Some(mut s) = m.row_data(i) {
                    if s.id == id {
                        s.cover = img;
                        m.set_row_data(i, s);
                        break;
                    }
                }
            }
        });
    }
}

/// 填充「我的最爱」列表（收藏时间倒序）。封面按行 id 回填，不重建模型。
fn refresh_liked(ui: &crate::AppWindow) {
    let entries = crate::persist::liked::list_songs();
    let rows: Vec<crate::Song> = entries
        .iter()
        .map(|e| crate::Song {
            id: e.id as u32 as i32,
            name: e.name.as_str().into(),
            artist: e.artist.as_str().into(),
            album: e.album.as_str().into(),
            cover: crate::load::cover_now(&e.cover),
            duration: e.duration.as_str().into(),
            duration_sec: e.duration_sec,
            url: e.cover.as_str().into(),
            liked: true,
            bad: is_bad_song(e.id as u32 as i32),
        })
        .collect();
    ui.set_liked_songs(crate::load::model(rows));
    for e in entries {
        if e.cover.is_empty() {
            continue;
        }
        let id = e.id as u32 as i32;
        let w = ui.as_weak();
        crate::load::cover_warm(&e.cover, cache::CoverType::Song, move |img| {
            let Some(u) = w.upgrade() else { return; };
            let m = u.get_liked_songs();
            for i in 0..m.row_count() {
                if let Some(mut s) = m.row_data(i) {
                    if s.id == id {
                        s.cover = img;
                        m.set_row_data(i, s);
                        break;
                    }
                }
            }
        });
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn queue_entry_of(s: &crate::Song) -> crate::persist::queue::QueueEntry {
    crate::persist::queue::QueueEntry {
        id: s.id as u32 as i64,
        name: s.name.to_string(),
        artist: s.artist.to_string(),
        album: s.album.to_string(),
        cover: s.url.to_string(),
        duration: s.duration.to_string(),
        duration_sec: s.duration_sec,
    }
}

/// 队列快照落盘：条目 + 当前曲目 + 进度。优先 ctx 队列（点歌/换队列时已同步），
/// 为空时回退 UI 模型（启动早期尚未点歌）。
fn persist_queue(ui: &crate::AppWindow, ctx: &MobileCtx) {
    let mut items: Vec<crate::persist::queue::QueueEntry> =
        ctx.queue.borrow().iter().map(queue_entry_of).collect();
    if items.is_empty() {
        items = read_ui_queue(ui).iter().map(queue_entry_of).collect();
    }
    if items.is_empty() {
        return;
    }
    crate::persist::queue::save(
        items,
        ui.get_current_song().id as u32 as i64,
        ui.get_progress(),
    );
    ctx.last_qsave.set(now_secs());
}

/// 启动恢复：按 queue.json 重建队列与当前曲目（不自动播放），封面后台补图。
fn restore_queue(ctx: &Rc<MobileCtx>) {
    let Some(st) = crate::persist::queue::state() else { return; };
    let Some(ui) = ctx.ui.upgrade() else { return; };
    if st.items.is_empty() {
        return;
    }
    let rows: Vec<crate::Song> = st
        .items
        .iter()
        .map(|e| crate::Song {
            id: e.id as u32 as i32,
            name: e.name.as_str().into(),
            artist: e.artist.as_str().into(),
            album: e.album.as_str().into(),
            cover: crate::load::cover_now(&e.cover),
            duration: e.duration.as_str().into(),
            duration_sec: e.duration_sec,
            url: e.cover.as_str().into(),
            liked: crate::persist::liked::is_liked(e.id),
            bad: is_bad_song(e.id as u32 as i32),
        })
        .collect();
    *ctx.queue.borrow_mut() = rows.clone();
    let idx = rows
        .iter()
        .position(|s| s.id as u32 as i64 == st.current)
        .unwrap_or(0);
    ctx.cur.set(idx);
    ui.set_queue(crate::load::model(rows.clone()));
    let song = rows[idx].clone();
    ui.set_current_song(song.clone());
    ui.set_liked(crate::persist::liked::is_liked(song.id as u32 as i64));
    ui.set_progress(st.progress.clamp(0.0, 1.0));
    ui.set_playing(false);
    ui.set_cover_color(cover_color_of(&ui, ctx, &song));
    log::info!("[mobile] 恢复播放队列 {} 首，当前 id={}", rows.len(), song.id);

    // 封面补图：完成后按行 id 回写队列，并同步当前曲目封面
    for e in &st.items {
        if e.cover.is_empty() || crate::load::cover_now(&e.cover).size().width > 0 {
            continue;
        }
        let id = e.id as u32 as i32;
        let w = ui.as_weak();
        crate::load::cover_warm(&e.cover, cache::CoverType::Song, move |img| {
            if img.size().width == 0 {
                return;
            }
            let Some(u) = w.upgrade() else { return; };
            let m = u.get_queue();
            for i in 0..m.row_count() {
                if let Some(mut s) = m.row_data(i) {
                    if s.id == id {
                        s.cover = img.clone();
                        m.set_row_data(i, s);
                        break;
                    }
                }
            }
            let mut cur = u.get_current_song();
            if cur.id == id {
                cur.cover = img;
                u.set_current_song(cur);
            }
        });
    }
}

/// 播放指定歌曲：解析元数据 → 同步队列 → 更新 UI → 后台取音频并播放。
/// auto=true 表示由"播完自动切歌/跳过"发起，失败时会继续切下一首。
fn play_song(ctx: &Rc<MobileCtx>, id: i32, auto: bool) {
    play_song_at(ctx, id, 0.0, auto);
}

/// 同 play_song，但从 start（0~1 进度比例）开始：用于恢复上次播放位置。
fn play_song_at(ctx: &Rc<MobileCtx>, id: i32, start: f32, auto: bool) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    ctx.auto_next.set(auto);
    if !auto {
        ctx.auto_skip.set(0);
    }
    let q = queue_snapshot(ctx, &ui);

    let mut idx = q.iter().position(|s| s.id == id);
    let mut song = if let Some(i) = idx {
        q[i].clone()
    } else {
        // 队列未命中：按 UI 各列表模型 → 首页数据池依次解析
        match find_song_anywhere(&ui, id) {
            Some(s) => s,
            None => {
                eprintln!("[mobile] 未找到歌曲元数据: id={id}");
                return;
            }
        }
    };

    // 封面补图：队列行可能尚无封面（空图）时用 UI 线程内存/磁盘缓存补齐
    if song.cover.size().width == 0 {
        let img = crate::load::cover_now(&song.url);
        if img.size().width > 0 {
            song.cover = img;
        }
    }

    // 不在队列则追加，并同步回 UI 模型
    if idx.is_none() {
        ctx.queue.borrow_mut().push(song.clone());
        idx = Some(ctx.queue.borrow().len().saturating_sub(1));
        ui.set_queue(crate::load::model(ctx.queue.borrow().clone()));
    }
    ctx.cur.set(idx.unwrap_or(0));
    log::info!("[mobile] play_song id={} name={}", song.id, song.name);

    ui.set_current_song(song.clone());
    // 新播放请求：清除上一次的错误提示与失败标志；点击该曲视为重试，先清坏歌标记
    ui.set_player_error(false);
    player::clear_failure();
    PLAY_FAILED.store(false, Ordering::SeqCst);
    crate::persist::bad::unmark(song.id as i64);
    sync_bad(&ui);
    // 收藏态：切歌后按当前曲目刷新播放层爱心
    crate::persist::liked::sync_liked(&ui);
    // 最近播放：去重前置并落盘，同步刷新最近页模型
    crate::persist::recent::record(
        song.id as u32 as i64,
        song.name.to_string(),
        song.artist.to_string(),
        song.album.to_string(),
        song.url.to_string(),
        song.duration.to_string(),
        song.duration_sec,
        0,
    );
    refresh_recent(&ui);
    // 播放层环境背景：取封面主色（取色失败回退主题色）
    let cover_color = cover_color_of(&ui, ctx, &song);
    ui.set_cover_color(cover_color);
    ui.set_progress(start.clamp(0.0, 1.0));
    ui.set_playing(true);
    // 音频尚未就绪：先进入加载态，避免"无声音也无提示"
    ui.set_player_loading(true);
    // 切歌即落盘：队列/当前曲目/进度
    persist_queue(&ui, ctx);

    // 即刻停旧流并进入切换过渡态，点击反馈不闪 UI
    player::stop();
    player::begin_switch();

    // 播放请求序号：快速连点不同歌曲时，先发后到的音频不得覆盖后点的歌
    let play_seq = ctx.play_seq.clone();
    let seq = play_seq.fetch_add(1, Ordering::SeqCst) + 1;

    // Slint int 是 i32：大 ID 存为负数，须经 u32 往返还原真实网易云 ID
    let uid = song.id as u32 as i64;

    // 歌词：先清空上一首，再与音频并行拉取；失败/无歌词只留空（播放不受影响）。
    // 用 song_seq（仅切歌自增）校验，拖动进度 seek 不会让在途歌词作废。
    ui.set_lyrics(crate::load::model::<crate::LyricRow>(Vec::new()));
    let song_token = ctx.song_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let lyric_client = ctx.client.clone();
    let lyric_ui = ctx.ui.clone();
    let lyric_seq = ctx.song_seq.clone();
    ctx.rt.spawn(async move {
        let parsed = match crate::network::service::song_lyric(&lyric_client, uid).await {
            Ok(v) => v
                .get("lrc")
                .and_then(|x| x.get("lyric"))
                .and_then(|x| x.as_str())
                .map(crate::core::lyrics::parse_lrc)
                .unwrap_or_default(),
            Err(e) => {
                log::warn!("[mobile] 歌词获取失败: id={uid} err={e}");
                return;
            }
        };
        if parsed.is_empty() {
            log::warn!("[mobile] 歌词为空: id={uid}");
        }
        // 期间已切歌：不写入内存表，也不回填 UI
        if lyric_seq.load(Ordering::SeqCst) != song_token {
            return;
        }
        // 与桌面共用同一份内存表（供播放进度定位）
        *crate::core::lyrics::LYRICS.lock().unwrap() = parsed.clone();
        let n = parsed.len();
        let rows: Vec<crate::LyricRow> = parsed
            .iter()
            .enumerate()
            .map(|(i, (t, text))| {
                let start = (t * 1000.0) as i32;
                crate::LyricRow {
                    text: text.as_str().into(),
                    start_ms: start,
                    end_ms: if i + 1 < n {
                        (parsed[i + 1].0 * 1000.0) as i32
                    } else {
                        start + 5000
                    },
                }
            })
            .collect();
        let _ = slint::invoke_from_event_loop(move || {
            // 期间已切歌：本次歌词作废
            if lyric_seq.load(Ordering::SeqCst) != song_token {
                return;
            }
            if let Some(u) = lyric_ui.upgrade() {
                u.set_lyrics(crate::load::model(rows));
            }
        });
    });

    let dur_sec = song.duration_sec.max(0.0);
    // 起始偏移（秒）：末尾留 1 秒，避免 ratio≈1 时立即播完触发自动切歌
    let start_off = (dur_sec * start.clamp(0.0, 1.0)).min((dur_sec - 1.0).max(0.0));
    let name = song.name.to_string();
    let artist = song.artist.to_string();
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    ctx.rt.spawn(async move {
        let source = crate::fetch::open_audio(client, uid, name, artist, dur_sec as u64).await;
        // 期间又点了别的歌：本次结果作废（不播放、不改 playing/过渡态）
        if play_seq.load(Ordering::SeqCst) != seq {
            return;
        }
        match source {
            Some(crate::fetch::AudioSource::Bytes(b)) => {
                player::play_at(b, dur_sec, start_off);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                        u.set_player_loading(false);
                    }
                });
            }
            Some(crate::fetch::AudioSource::Stream(buf)) => {
                // 首块数据已到达：立即出声，后台继续下载并写缓存
                player::play_stream(buf, dur_sec, start_off);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                        u.set_player_loading(false);
                    }
                });
            }
            None => {
                // 无法获取音频：置失败标志，由 250ms 轮询统一做提示/标记/自动续播
                PLAY_FAILED.store(true, Ordering::SeqCst);
                log::warn!("[mobile] 音频获取失败: id={uid}");
            }
        }
    });
}

/// 拖动进度：保持当前曲目，从 ratio 位置重新取流续播（旧请求按序号作废）。
fn seek_song(ctx: &Rc<MobileCtx>, ratio: f32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let ratio = ratio.clamp(0.0, 1.0);
    let song = ui.get_current_song();
    if song.name.is_empty() {
        return;
    }
    let uid = song.id as u32 as i64;
    let dur_sec = song.duration_sec.max(0.0);
    if dur_sec <= 0.0 {
        return;
    }
    // 末尾留 1 秒，避免 ratio=1.0 时 offset 等于总时长导致立即播完并自动切歌
    let offset = (dur_sec * ratio).min((dur_sec - 1.0).max(0.0));
    let name = song.name.to_string();
    let artist = song.artist.to_string();

    // 先反馈预览位置，再停旧流进入切换过渡态
    ui.set_progress(ratio);
    ui.set_player_loading(true);
    ui.set_player_error(false);
    player::clear_failure();
    PLAY_FAILED.store(false, Ordering::SeqCst);
    player::stop();
    player::begin_switch();

    let play_seq = ctx.play_seq.clone();
    let seq = play_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    ctx.rt.spawn(async move {
        let source = crate::fetch::open_audio(client, uid, name, artist, dur_sec as u64).await;
        // 期间又点歌/再次拖动：本次结果作废
        if play_seq.load(Ordering::SeqCst) != seq {
            return;
        }
        match source {
            Some(crate::fetch::AudioSource::Bytes(b)) => {
                player::play_at(b, dur_sec, offset);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                        u.set_player_loading(false);
                    }
                });
            }
            Some(crate::fetch::AudioSource::Stream(buf)) => {
                player::play_stream(buf, dur_sec, offset);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                        u.set_player_loading(false);
                    }
                });
            }
            None => {
                PLAY_FAILED.store(true, Ordering::SeqCst);
                log::warn!("[mobile] seek 音频获取失败: id={uid} offset={offset}");
            }
        }
    });
}

/// 该曲是否已被标记为无法播放。
fn is_bad_song(id: i32) -> bool {
    crate::persist::bad::is_bad(id as i64)
}

/// 坏歌标记同步到所有列表模型（播放队列/最近/最爱/搜索/详情）与当前曲目。
fn sync_bad(ui: &crate::AppWindow) {
    for m in [
        ui.get_queue(),
        ui.get_recent_songs(),
        ui.get_liked_songs(),
        ui.get_search_songs(),
        ui.get_detail_songs(),
    ] {
        apply_bad_to_model(&m);
    }
    let mut cur = ui.get_current_song();
    let bad = is_bad_song(cur.id);
    if cur.bad != bad {
        cur.bad = bad;
        ui.set_current_song(cur);
    }
}

/// 按持久化标记刷新模型行的 bad 字段（仅在有变化时 set_row_data）。
fn apply_bad_to_model(m: &slint::ModelRc<crate::Song>) {
    for i in 0..m.row_count() {
        if let Some(mut s) = m.row_data(i) {
            let bad = is_bad_song(s.id);
            if s.bad != bad {
                s.bad = bad;
                m.set_row_data(i, s);
            }
        }
    }
}

/// 播放失败统一处理（UI 线程）：标记坏歌 + 进度条上方错误提示；
/// 若本次由自动切歌发起则继续跳下一首（防死循环：最多队列长度次）。
fn on_play_failed(ctx: &Rc<MobileCtx>) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let song = ui.get_current_song();
    if song.name.is_empty() {
        return;
    }
    crate::persist::bad::mark(song.id as i64);
    sync_bad(&ui);
    player::end_switch();
    ui.set_player_loading(false);
    ui.set_playing(false);
    ui.set_player_error(true);
    log::warn!("[mobile] 播放失败，已标记: id={} name={}", song.id, song.name);

    if ctx.auto_next.get() {
        let n = queue_snapshot(ctx, &ui).len();
        if ctx.auto_skip.get() + 1 < n {
            ctx.auto_skip.set(ctx.auto_skip.get() + 1);
            step_song(ctx, 1, true);
        } else {
            ctx.auto_skip.set(0);
        }
    }
}

/// 按播放模式切换上一首/下一首：dir=1 下一首，dir=-1 上一首。
/// 已标记的坏歌自动跳过；auto=true 表示由播完自动触发。
fn step_song(ctx: &Rc<MobileCtx>, dir: i32, auto: bool) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let q = queue_snapshot(ctx, &ui);
    let n = q.len();
    if n == 0 {
        ui.set_playing(false);
        return;
    }
    let cur_id = ui.get_current_song().id;
    let cur = q
        .iter()
        .position(|s| s.id == cur_id)
        .unwrap_or_else(|| ctx.cur.get().min(n - 1));
    let target = if ctx.mode.get() == 1 && !is_bad_song(q[cur].id) {
        // 单曲循环：上一首/下一首/播完自动切均重播当前（当前曲本身不是坏歌时）
        cur
    } else {
        // 列表循环 / 顺序循环 / 单曲循环遇坏歌：按序移动并跳过已标记坏歌
        let mut t = cur;
        let mut found = None;
        for _ in 0..n {
            t = (((t as i32 + dir) % n as i32) + n as i32) as usize % n;
            if !is_bad_song(q[t].id) {
                found = Some(t);
                break;
            }
        }
        match found {
            Some(t) => t,
            None => {
                // 整队都是坏歌：停止播放并提示
                player::stop();
                player::end_switch();
                ui.set_playing(false);
                ui.set_player_loading(false);
                ui.set_player_error(true);
                return;
            }
        }
    };
    ctx.cur.set(target);
    play_song(ctx, q[target].id, auto);
}

/// 安装移动端全部回调与进度轮询。
pub fn setup(ui: &crate::AppWindow, rt: tokio::runtime::Handle, client: Arc<NeteaseClient>) {
    ui.set_play_mode(0);
    ui.set_playing(false);

    // 启动即应用用户配置（深浅色 / 设置页各控件状态 / 缓存与数据目录）；
    // 主题色切换功能已废除，配色固定为默认，不再从配置读取。
    let cfg = AppConfig::load();
    ui.set_dark(cfg.app.dark);
    ui.set_settings_language(if cfg.app.language_code == "en" { 1 } else { 0 });
    ui.set_settings_quality(cfg.play.quality_index.clamp(0, 2));
    ui.set_settings_source(cfg.play.source_index.clamp(0, 4));
    ui.set_cache_usage(format_usage(cache::total_usage()).into());
    ui.set_data_dir(
        crate::persist::config::data_dir()
            .display()
            .to_string()
            .into(),
    );
    // 设置页新增项（global MobileSettings）：版本号 / 自动播放 / 缓存开关与上限
    {
        let g = ui.global::<crate::MobileSettings>();
        g.set_app_version(env!("CARGO_PKG_VERSION").into());
        g.set_auto_play(cfg.play.auto_play);
        g.set_cache_enabled(cfg.cache.enabled);
        g.set_cache_limit_index(cache_limit_index(cfg.app.cache_limit_mb));
    }
    // 启动即填充最近播放（记录已在入口 load_recent 恢复）
    refresh_recent(ui);
    // 坏歌标记同步到 UI 全局（列表行显示「无法播放」）
    sync_bad(ui);

    let ctx = Rc::new(MobileCtx {
        ui: ui.as_weak(),
        rt,
        client,
        queue: RefCell::new(read_ui_queue(ui)),
        cur: Cell::new(0),
        mode: Cell::new(0),
        search_seq: Arc::new(AtomicU64::new(0)),
        search_kw: RefCell::new(String::new()),
        detail_seq: Arc::new(AtomicU64::new(0)),
        browse_seq: Arc::new(AtomicU64::new(0)),
        browse_offset: Arc::new(AtomicU64::new(0)),
        play_seq: Arc::new(AtomicU64::new(0)),
        song_seq: Arc::new(AtomicU64::new(0)),
        cfg: RefCell::new(cfg),
        cover_colors: RefCell::new(HashMap::new()),
        last_qsave: Cell::new(0),
        auto_next: Cell::new(false),
        auto_skip: Cell::new(0),
    });

    // 启动恢复上次队列（有 queue.json 时首页加载不会覆盖队列与当前曲目）
    restore_queue(&ctx);

    // 设置页「自动播放」：恢复队列后自动续播上次歌曲（坏歌不自动播放，避免启动即报错）
    if ctx.cfg.borrow().play.auto_play {
        if let Some(u) = ctx.ui.upgrade() {
            let song = u.get_current_song();
            if song.id != 0 && !is_bad_song(song.id) {
                let start = u.get_progress();
                log::info!("[mobile] 自动播放：恢复上次歌曲 id={}", song.id);
                play_song_at(&ctx, song.id, start, false);
            }
        }
    }

    // 「最近」页的我的最爱 Tab 首屏填充（收藏数据为本地快照，无网络阻塞）
    if let Some(u) = ctx.ui.upgrade() {
        refresh_liked(&u);
        // 搜索历史首屏填充
        push_search_history(&u);
    }

    // ---- 首页点歌（详情页点歌：队列不含该曲时用详情列表整体替换队列）----
    let c = ctx.clone();
    ui.on_song_played(move |id| {
        if let Some(u) = c.ui.upgrade() {
            let detail = read_song_model(&u.get_detail_songs());
            if detail.iter().any(|s| s.id == id) {
                let ui_queue = read_ui_queue(&u);
                let in_queue = ui_queue.iter().any(|s| s.id == id)
                    || c.queue.borrow().iter().any(|s| s.id == id);
                if !in_queue {
                    *c.queue.borrow_mut() = detail.clone();
                    u.set_queue(crate::load::model(detail));
                    c.cur.set(0);
                }
            }
        }
        play_song(&c, id, false);
    });

    // 歌单/专辑打开：进入详情页
    let c = ctx.clone();
    ui.on_playlist_opened(move |id| open_detail(&c, id, 0));

    // ---- 搜索 ----
    let c = ctx.clone();
    ui.on_search_submitted(move |keyword, ty| start_search(&c, keyword.as_str(), ty));

    // ---- 搜索历史：单条删除 / 清空（持久化后刷新）----
    let c = ctx.clone();
    ui.on_search_history_remove(move |w| {
        crate::persist::history::remove(w.as_str());
        if let Some(u) = c.ui.upgrade() {
            push_search_history(&u);
        }
    });

    let c = ctx.clone();
    ui.on_search_history_clear(move || {
        crate::persist::history::clear();
        if let Some(u) = c.ui.upgrade() {
            push_search_history(&u);
        }
    });

    let c = ctx.clone();
    ui.on_search_type_changed(move |ty| {
        if let Some(u) = c.ui.upgrade() {
            u.set_search_type(ty);
        }
        // 保留旧结果直到新结果到达（期间 loading=true）
        let kw = c.search_kw.borrow().clone();
        if !kw.is_empty() {
            start_search(&c, &kw, ty);
        }
    });

    let c = ctx.clone();
    ui.on_album_opened(move |id| open_detail(&c, id, 1));

    let c = ctx.clone();
    ui.on_radio_opened(move |id| open_detail(&c, id, 3));

    // ---- 每日推荐：进入每日推荐歌单详情（kind=4，读 HERO_POOL）----
    let c = ctx.clone();
    ui.on_hero_opened(move || open_detail(&c, 0, 4));

    // Hero 卡内单曲点击：直接播放
    let c = ctx.clone();
    ui.on_hero_song_played(move |id| play_song(&c, id, false));

    // ---- 分类浏览：打开 / 分类切换 / 加载更多 ----
    let c = ctx.clone();
    ui.on_browse_opened(move |kind| open_browse(&c, kind));
    let c = ctx.clone();
    ui.on_browse_cat_changed(move |idx| browse_cat_changed(&c, idx));
    let c = ctx.clone();
    ui.on_browse_load_more(move || browse_load_more(&c));
    let c = ctx.clone();
    ui.on_browse_retry(move || browse_retry(&c));

    // ---- 详情页 ----
    // 返回：作废在途请求并关闭详情
    let c = ctx.clone();
    ui.on_detail_back(move || {
        c.detail_seq.fetch_add(1, Ordering::SeqCst);
        if let Some(u) = c.ui.upgrade() {
            u.set_detail_open(false);
        }
    });

    // 播放全部：详情列表整体成为队列，从第一首开始
    let c = ctx.clone();
    ui.on_detail_play_all(move || {
        let Some(u) = c.ui.upgrade() else { return; };
        let songs = read_song_model(&u.get_detail_songs());
        if songs.is_empty() {
            return;
        }
        let first = songs[0].id;
        *c.queue.borrow_mut() = songs.clone();
        u.set_queue(crate::load::model(songs));
        c.cur.set(0);
        play_song(&c, first, false);
    });

    // ---- 播放控制 ----
    let c = ctx.clone();
    ui.on_player_toggle(move || {
        let Some(u) = c.ui.upgrade() else { return; };
        // 加载中点击 = 取消本次加载：作废在途请求并复位，避免稍后"自己响起来"
        if u.get_player_loading() {
            c.play_seq.fetch_add(1, Ordering::SeqCst);
            player::stop();
            u.set_player_loading(false);
            u.set_playing(false);
            return;
        }
        if player::active() {
            player::toggle();
            let (_, playing) = player::status();
            u.set_playing(playing);
            // 暂停/继续时保存一次进度（定时保存之外的兜底）
            persist_queue(&u, &c);
            return;
        }
        let cur_id = u.get_current_song().id;
        // 冷启动恢复后首次播放：从上次保存的进度续播（接近结尾则从头开始）
        let mut start = if cur_id != 0 { u.get_progress() } else { 0.0 };
        if start > 0.95 {
            start = 0.0;
        }
        // 尚无当前曲目时：取队列首曲（首页数据已到达时队列非空）
        let id = if cur_id != 0 {
            cur_id
        } else {
            let q = queue_snapshot(&c, &u);
            match q.first() {
                Some(s) => s.id,
                None => return,
            }
        };
        drop(u);
        play_song_at(&c, id, start, false);
    });

    let c = ctx.clone();
    ui.on_player_next(move || step_song(&c, 1, false));
    let c = ctx.clone();
    ui.on_player_prev(move || step_song(&c, -1, false));

    // 播放层拖动 seek / 队列点播
    let c = ctx.clone();
    ui.on_player_seek(move |ratio| seek_song(&c, ratio));
    let c = ctx.clone();
    ui.on_queue_played(move |id| play_song(&c, id, false));

    let c = ctx.clone();
    ui.on_player_mode(move || {
        let next = (c.mode.get() + 1) % 3;
        c.mode.set(next);
        if let Some(u) = c.ui.upgrade() {
            u.set_play_mode(next);
        }
    });

    // 收藏/取消收藏当前歌曲：本地持久化并立即刷新爱心状态
    let c = ctx.clone();
    ui.on_player_like(move || {
        let Some(u) = c.ui.upgrade() else { return; };
        let song = u.get_current_song();
        if song.id == 0 {
            return;
        }
        let uid = song.id as u32 as i64;
        let meta = crate::persist::liked::LikedSong {
            id: uid,
            name: song.name.to_string(),
            artist: song.artist.to_string(),
            album: song.album.to_string(),
            cover: song.url.to_string(),
            duration: song.duration.to_string(),
            duration_sec: song.duration_sec,
            ts: 0,
        };
        let liked = crate::persist::liked::toggle_liked(uid, Some(meta));
        u.set_liked(liked);
        // 「最近」页的我的最爱 Tab 同步刷新
        refresh_liked(&u);
    });

    // ---- 最近播放：全部清空（无确认，与桌面一致）----
    let c = ctx.clone();
    ui.on_recent_clear(move || {
        crate::persist::recent::clear();
        if let Some(u) = c.ui.upgrade() {
            refresh_recent(&u);
        }
    });

    // ---- 我的最爱（「最近」页 Tab）：点歌 / 播放全部 ----
    let c = ctx.clone();
    ui.on_liked_played(move |id| play_song(&c, id, false));

    let c = ctx.clone();
    ui.on_liked_play_all(move || {
        let Some(u) = c.ui.upgrade() else { return; };
        let songs = read_song_model(&u.get_liked_songs());
        if songs.is_empty() {
            return;
        }
        let first = songs[0].id;
        *c.queue.borrow_mut() = songs.clone();
        u.set_queue(crate::load::model(songs));
        c.cur.set(0);
        play_song(&c, first, false);
    });

    // ---- 歌曲操作抽屉（长按 / ⋯）：打开 / 播放 / 下一首播放 / 收藏切换 / 移除 ----
    let c = ctx.clone();
    ui.on_song_more(move |id, source| {
        let Some(u) = c.ui.upgrade() else { return; };
        let Some(song) = find_song_anywhere(&u, id) else { return; };
        u.set_sheet_song(song);
        u.set_sheet_liked(crate::persist::liked::is_liked(id as u32 as i64));
        u.set_sheet_source(source);
        let label: slint::SharedString = match source {
            2 => "从最近中移除".into(),
            5 => "从队列中移除".into(),
            _ => "".into(),
        };
        u.set_sheet_remove_label(label);
        u.set_sheet_open(true);
    });

    let c = ctx.clone();
    ui.on_sheet_play(move |id| play_song(&c, id, false));

    // 下一首播放：不在队列时插到当前曲目之后
    let c = ctx.clone();
    ui.on_sheet_play_next(move |id| {
        let Some(u) = c.ui.upgrade() else { return; };
        let Some(song) = find_song_anywhere(&u, id) else { return; };
        let mut q = queue_snapshot(&c, &u);
        if q.iter().any(|s| s.id == song.id) {
            return;
        }
        let at = (c.cur.get() + 1).min(q.len());
        q.insert(at, song);
        *c.queue.borrow_mut() = q.clone();
        u.set_queue(crate::load::model(q));
        persist_queue(&u, &c);
    });

    let c = ctx.clone();
    ui.on_sheet_toggle_like(move |id| {
        let Some(u) = c.ui.upgrade() else { return; };
        let Some(song) = find_song_anywhere(&u, id) else { return; };
        let uid = id as u32 as i64;
        let meta = crate::persist::liked::LikedSong {
            id: uid,
            name: song.name.to_string(),
            artist: song.artist.to_string(),
            album: song.album.to_string(),
            cover: song.url.to_string(),
            duration: song.duration.to_string(),
            duration_sec: song.duration_sec,
            ts: 0,
        };
        crate::persist::liked::toggle_liked(uid, Some(meta));
        refresh_liked(&u);
        // 当前播放曲目被切换收藏时同步播放层爱心
        crate::persist::liked::sync_liked(&u);
    });

    // 移除：按抽屉来源删除（最近记录 / 播放队列）
    let c = ctx.clone();
    ui.on_sheet_remove(move |id| {
        let Some(u) = c.ui.upgrade() else { return; };
        match u.get_sheet_source() {
            2 => {
                crate::persist::recent::remove(id as u32 as i64);
                refresh_recent(&u);
            }
            5 => {
                let mut q = queue_snapshot(&c, &u);
                let Some(pos) = q.iter().position(|s| s.id == id) else { return; };
                q.remove(pos);
                let cur = c.cur.get();
                if pos < cur {
                    c.cur.set(cur - 1);
                } else if pos == cur {
                    c.cur.set(cur.min(q.len().saturating_sub(1)));
                }
                *c.queue.borrow_mut() = q.clone();
                u.set_queue(crate::load::model(q));
                persist_queue(&u, &c);
            }
            _ => {}
        }
    });

    // ---- 设置页：改内存配置 → 写盘 → 立即应用 ----
    let c = ctx.clone();
    ui.on_dark_changed(move |v| {
        c.cfg.borrow_mut().app.dark = v;
        c.cfg.borrow().save();
        if let Some(u) = c.ui.upgrade() {
            u.set_dark(v);
        }
    });

    // 主题色切换功能已废除：不再注册 on_theme_changed，配色固定为默认。

    // 移动端 UI 文案暂固定中文：只持久化语言选择，不调用桌面专用的 i18n::load
    let c = ctx.clone();
    ui.on_settings_language_changed(move |i| {
        let i = i.clamp(0, 1);
        let code = if i == 1 { "en" } else { "zh" };
        c.cfg.borrow_mut().app.language_code = code.to_string();
        c.cfg.borrow().save();
        if let Some(u) = c.ui.upgrade() {
            u.set_settings_language(i);
        }
    });

    let c = ctx.clone();
    ui.on_settings_quality_changed(move |i| {
        let i = i.clamp(0, 2);
        c.cfg.borrow_mut().play.quality_index = i;
        c.cfg.borrow().save();
        if let Some(u) = c.ui.upgrade() {
            u.set_settings_quality(i);
        }
    });

    let c = ctx.clone();
    ui.on_settings_source_changed(move |i| {
        let i = i.clamp(0, 4);
        c.cfg.borrow_mut().play.source_index = i;
        c.cfg.borrow().save();
        if let Some(u) = c.ui.upgrade() {
            u.set_settings_source(i);
        }
    });

    let c = ctx.clone();
    ui.on_settings_clear_cache(move || {
        let freed = cache::clear_all();
        log::info!("[mobile] 已清空缓存，释放 {freed} 字节");
        if let Some(u) = c.ui.upgrade() {
            u.set_cache_usage(format_usage(cache::total_usage()).into());
        }
    });

    // ---- 设置页新增项（global MobileSettings）：自动播放 / 缓存开关与上限 / 关于与更新 ----
    let c = ctx.clone();
    ui.global::<crate::MobileSettings>().on_auto_play_changed(move |on| {
        c.cfg.borrow_mut().play.auto_play = on;
        c.cfg.borrow().save();
        log::info!("[mobile] 自动播放 → {on}");
    });

    let c = ctx.clone();
    ui.global::<crate::MobileSettings>().on_cache_enabled_changed(move |on| {
        c.cfg.borrow_mut().cache.enabled = on;
        c.cfg.borrow().save();
        cache::init(&c.cfg.borrow());
        log::info!("[mobile] 边听边缓存 → {on}");
    });

    let c = ctx.clone();
    ui.global::<crate::MobileSettings>().on_cache_limit_changed(move |idx| {
        let mb = cache_limit_mb(idx);
        c.cfg.borrow_mut().app.cache_limit_mb = mb;
        c.cfg.borrow().save();
        cache::init(&c.cfg.borrow());
        cache::enforce_limit_now();
        if let Some(u) = c.ui.upgrade() {
            u.global::<crate::MobileSettings>().set_cache_limit_index(idx);
            u.set_cache_usage(format_usage(cache::total_usage()).into());
        }
        log::info!("[mobile] 缓存上限 → {mb}MB");
    });

    // 检查更新：后台查询 GitHub Release，结果回投 UI 线程写入 MobileSettings
    let c = ctx.clone();
    ui.global::<crate::MobileSettings>().on_check_update(move || {
        let Some(u) = c.ui.upgrade() else { return; };
        {
            let g = u.global::<crate::MobileSettings>();
            if g.get_update_checking() {
                return;
            }
            g.set_update_checking(true);
            g.set_update_version("".into());
            g.set_update_notes("".into());
            g.set_update_message("".into());
        }
        let ui_w = c.ui.clone();
        c.rt.spawn(async move {
            let r = check_update_latest().await;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = ui_w.upgrade() else { return; };
                let g = u.global::<crate::MobileSettings>();
                g.set_update_checking(false);
                match r {
                    Ok(Some((ver, notes))) => {
                        log::info!("[mobile] 发现新版本 v{ver}");
                        g.set_update_version(ver.into());
                        g.set_update_notes(notes.into());
                    }
                    Ok(None) => g.set_update_message("已是最新版本".into()),
                    Err(e) => {
                        log::warn!("[mobile] 检查更新失败: {e}");
                        g.set_update_message(format!("检查更新失败：{e}").into());
                    }
                }
            });
        });
    });

    ui.global::<crate::MobileSettings>().on_open_update_page(move || {
        crate::mobile::entry::open_url(UPDATE_PAGE);
    });

    // MV 分类条目：移动端未集成视频解码栈，交给系统浏览器打开网易云 MV 页
    ui.global::<crate::MobileSettings>().on_mv_opened(move |id| {
        let full = crate::core::ids::resolve(4, id) as u64;
        let url = format!("https://music.163.com/mv/{full}");
        log::info!("[mobile] 打开 MV: {url}");
        crate::mobile::entry::open_url(&url);
    });

    // ---- 250ms 进度轮询 ----
    let c = ctx.clone();
    let was_playing = Rc::new(Cell::new(false));
    let timer = Rc::new(slint::Timer::default());
    let was = was_playing.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        move || {
            let Some(ui) = c.ui.upgrade() else { return; };
            // 播放失败（取流失败/解码失败/流中断）：统一提示 + 标记坏歌 + 自动续播
            if PLAY_FAILED.swap(false, Ordering::SeqCst) || player::take_failure() {
                was.set(false);
                on_play_failed(&c);
                return;
            }
            if player::switching() {
                return;
            }
            if player::active() {
                let (p, playing) = player::status();
                ui.set_progress(p);
                ui.set_playing(playing);
                was.set(playing);
                // 每 15 秒保存一次进度：应用被系统回收后可恢复到近似位置
                if now_secs().saturating_sub(c.last_qsave.get()) >= 15 {
                    persist_queue(&ui, &c);
                }
            } else if was.get() && player::finished() {
                // 正常播完：自动切下一首（失败/切换中不触发；跳过已标记坏歌）
                was.set(false);
                c.auto_skip.set(0);
                step_song(&c, 1, true);
            }
        },
    );
    // Timer 需要与事件循环同生命周期；主动持有到进程结束
    std::mem::forget(timer);
}
