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
use std::sync::Arc;

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
    /// 播放请求序号：每次点歌自增；旧请求晚到的音频字节直接丢弃，避免覆盖后点的歌。
    pub play_seq: Arc<AtomicU64>,
    /// 当前曲目序号：仅在切歌时自增；歌词请求按它校验（seek 不影响歌词）。
    pub song_seq: Arc<AtomicU64>,
    /// 用户配置快照（设置页修改后写盘；与桌面共用同一份 config.toml）
    pub cfg: RefCell<AppConfig>,
    /// 封面主色缓存（按 song.id）：避免每次切歌重复解码采样
    pub cover_colors: RefCell<HashMap<i32, slint::Color>>,
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
    }
}

/// 取色失败时的回退主色（顺序与设置页主题色一致）。
const FALLBACK_COVER_COLORS: [u32; 4] = [0x6c5ce7, 0x0ea371, 0xea580c, 0x3b82f6];

/// 封面主色：按 song.id 缓存；无图/解码失败回退当前主题色，保证播放层背景始终有色。
fn cover_color_of(ui: &crate::AppWindow, ctx: &MobileCtx, song: &crate::Song) -> slint::Color {
    if let Some(c) = ctx.cover_colors.borrow().get(&song.id).copied() {
        return c;
    }
    let color = cover_color_from_image(&crate::load::cover_now(&song.url)).unwrap_or_else(|| {
        let idx = ui.get_theme_color_index().clamp(0, 3) as usize;
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

/// 打开歌单/专辑详情：序号自增 → 置加载态 → 后台拉取 → UI 线程按序号防串回填。
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

    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    let seq_now = ctx.detail_seq.clone();
    // Slint int 是 i32：大 ID 存为负数，须经 u32 往返还原真实网易云 ID
    let uid = id as u32 as u64;
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
                    }
                    let rows: Vec<crate::Song> = songs.iter().map(song_from_base).collect();
                    u.set_detail_songs(crate::load::model(rows));
                    u.set_detail_error("".into());
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

/// 播放指定歌曲：解析元数据 → 同步队列 → 更新 UI → 后台取音频并播放。
fn play_song(ctx: &Rc<MobileCtx>, id: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let q = queue_snapshot(ctx, &ui);

    let mut idx = q.iter().position(|s| s.id == id);
    let mut song = if let Some(i) = idx {
        q[i].clone()
    } else {
        // 队列未命中：首页池 → 搜索结果列表（搜索页行点击播放）
        match resolve_from_pools(id).or_else(|| find_in_model(&ui.get_search_songs(), id)) {
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

    ui.set_current_song(song.clone());
    // 播放层环境背景：取封面主色（取色失败回退主题色）
    let cover_color = cover_color_of(&ui, ctx, &song);
    ui.set_cover_color(cover_color);
    ui.set_progress(0.0);
    ui.set_playing(true);

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
    let name = song.name.to_string();
    let artist = song.artist.to_string();
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    ctx.rt.spawn(async move {
        let bytes = crate::fetch::fetch_song_bytes(&client, uid, &name, &artist, dur_sec as u64).await;
        // 期间又点了别的歌：本次结果作废（不 play_at、不改 playing/过渡态）
        if play_seq.load(Ordering::SeqCst) != seq {
            return;
        }
        match bytes {
            Some(b) => {
                player::play_at(b, dur_sec, 0.0);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                    }
                });
            }
            None => {
                player::end_switch();
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(false);
                    }
                });
                eprintln!("[mobile] 音频获取失败: id={uid} name={name}");
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
    player::stop();
    player::begin_switch();

    let play_seq = ctx.play_seq.clone();
    let seq = play_seq.fetch_add(1, Ordering::SeqCst) + 1;
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    ctx.rt.spawn(async move {
        let bytes = crate::fetch::fetch_song_bytes(&client, uid, &name, &artist, dur_sec as u64).await;
        // 期间又点歌/再次拖动：本次结果作废
        if play_seq.load(Ordering::SeqCst) != seq {
            return;
        }
        match bytes {
            Some(b) => {
                player::play_at(b, dur_sec, offset);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                    }
                });
            }
            None => {
                player::end_switch();
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(false);
                    }
                });
                log::warn!("[mobile] seek 音频获取失败: id={uid} offset={offset}");
            }
        }
    });
}

/// 按播放模式切换上一首/下一首：dir=1 下一首，dir=-1 上一首。
fn step_song(ctx: &Rc<MobileCtx>, dir: i32) {
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
    let target = if ctx.mode.get() == 1 {
        // 单曲循环：上一首/下一首均重播当前
        cur
    } else {
        // 列表循环 / 顺序循环：按序移动并回绕
        (((cur as i32 + dir) % n as i32) + n as i32) as usize % n
    };
    ctx.cur.set(target);
    play_song(ctx, q[target].id);
}

/// 安装移动端全部回调与进度轮询。
pub fn setup(ui: &crate::AppWindow, rt: tokio::runtime::Handle, client: Arc<NeteaseClient>) {
    ui.set_play_mode(0);
    ui.set_playing(false);

    // 启动即应用用户配置（深浅色 / 主题色 / 设置页各控件状态 / 缓存与数据目录）
    let cfg = AppConfig::load();
    ui.set_dark(cfg.app.dark);
    ui.set_theme_color_index(cfg.ui.theme_color_index.clamp(0, 3));
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
        play_seq: Arc::new(AtomicU64::new(0)),
        song_seq: Arc::new(AtomicU64::new(0)),
        cfg: RefCell::new(cfg),
        cover_colors: RefCell::new(HashMap::new()),
    });

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
        play_song(&c, id);
    });

    // 歌单/专辑打开：进入详情页
    let c = ctx.clone();
    ui.on_playlist_opened(move |id| open_detail(&c, id, 0));

    // ---- 搜索 ----
    let c = ctx.clone();
    ui.on_search_submitted(move |keyword, ty| start_search(&c, keyword.as_str(), ty));

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
        play_song(&c, first);
    });

    // ---- 播放控制 ----
    let c = ctx.clone();
    ui.on_player_toggle(move || {
        if player::active() {
            player::toggle();
            if let Some(u) = c.ui.upgrade() {
                let (_, playing) = player::status();
                u.set_playing(playing);
            }
            return;
        }
        let Some(u) = c.ui.upgrade() else { return; };
        let cur_id = u.get_current_song().id;
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
        play_song(&c, id);
    });

    let c = ctx.clone();
    ui.on_player_next(move || step_song(&c, 1));
    let c = ctx.clone();
    ui.on_player_prev(move || step_song(&c, -1));

    // 播放层拖动 seek / 队列点播
    let c = ctx.clone();
    ui.on_player_seek(move |ratio| seek_song(&c, ratio));
    let c = ctx.clone();
    ui.on_queue_played(move |id| play_song(&c, id));

    let c = ctx.clone();
    ui.on_player_mode(move || {
        let next = (c.mode.get() + 1) % 3;
        c.mode.set(next);
        if let Some(u) = c.ui.upgrade() {
            u.set_play_mode(next);
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

    let c = ctx.clone();
    ui.on_theme_changed(move |i| {
        let i = i.clamp(0, 3);
        c.cfg.borrow_mut().ui.theme_color_index = i;
        c.cfg.borrow().save();
        if let Some(u) = c.ui.upgrade() {
            u.set_theme_color_index(i);
        }
    });

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
            if player::switching() {
                return;
            }
            if player::active() {
                let (p, playing) = player::status();
                ui.set_progress(p);
                ui.set_playing(playing);
                was.set(playing);
            } else if was.get() && player::finished() {
                // 正常播完：自动切下一首（失败/切换中不触发）
                was.set(false);
                step_song(&c, 1);
            }
        },
    );
    // Timer 需要与事件循环同生命周期；主动持有到进程结束
    std::mem::forget(timer);
}
