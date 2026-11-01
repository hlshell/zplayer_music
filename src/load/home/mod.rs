//! 首页数据加载主模块：并行拉取网易云发现页各板块并填充 Slint 模型。
//! 在后台 tokio 任务中执行，通过 slint::invoke_from_event_loop 在 UI 线程上更新模型。
//! 封面：网络 URL 在后台并行下载 + 解码成 SharedPixelBuffer（Send），
//! 再在 UI 线程构造 slint::Image 填入模型（slint::Image 本身不是 Send）。

use crate::persist::config::AppConfig;
use crate::network::netease::client::NeteaseClient;
use crate::network::service;
use crate::core::song_util::{fmt_dur, song_cover_url, song_fields};
use covers::cov;
use crate::{Album, Artist, HeroSong, HotSong, NewSong, Playlist, Song};
use slint::{Image, Model, SharedString};
use std::sync::{Arc, Mutex};

pub mod covers;
mod playlists;
mod artists;
mod albums;
mod newsongs;
mod toptracks;

// ---- 供 main.rs 按原 crate::load::X 路径访问的公共符号 ----
pub use covers::{cover_now, cover_warm, fetch_covers, set_runtime, warm_rows};
use crate::persist::cache::CoverType;
pub use crate::core::song_util::{from, model, parse_tracks, to, SongBase};

// 各板块基础数据（不含 cover，cover 由 covers::fetch_covers 异步解码后填入 slint::Image）
#[derive(Clone)] pub(crate) struct HotBase { id: i32, rank: SharedString, name: SharedString, artist: SharedString, from: slint::Color, to: slint::Color, url: String }
#[derive(Clone)] pub(crate) struct PlaylistBase { id: i32, name: SharedString, plays: SharedString, from: slint::Color, to: slint::Color, url: String }
#[derive(Clone)] pub(crate) struct ArtistBase { id: i32, name: SharedString, initial: SharedString, from: slint::Color, to: slint::Color, url: String }
#[derive(Clone)] pub(crate) struct AlbumBase { id: i32, name: SharedString, artist: SharedString, year: SharedString, from: slint::Color, to: slint::Color, url: String }
#[derive(Clone)] pub(crate) struct NewSongBase { id: i32, name: SharedString, artist: SharedString, url: String }

/// Hero 详情页/播放全部共用的歌曲池：榜单全量 tracks（含封面 URL）。
/// 在 load_initial 中填充；on_hero_play_all / on_open_daily_detail 读取。
/// 内部用 Arc 包裹：读取方拿到的是 Arc 快照（仅原子加引用计数），
/// 避免点歌/开详情等高频路径整池深拷贝（几十首 SharedString）。
pub static HERO_POOL: std::sync::LazyLock<Mutex<Arc<Vec<SongBase>>>> =
    std::sync::LazyLock::new(|| Mutex::new(Arc::new(Vec::new())));

/// 榜单歌单封面 URL（Hero 卡/详情页头部背景，走封面缓存）。
pub static HERO_COVER_URL: Mutex<String> = Mutex::new(String::new());

/// 首页数据加载入口：并行拉取发现页各板块并填充模型。
/// 启动时若配置存在上次播放歌曲且队列包含该曲，则恢复为当前曲目（不自动播放）。
pub async fn load_initial(client: Arc<NeteaseClient>, weak: slint::Weak<crate::AppWindow>, cfg: AppConfig) {
    // 数据请求开始：置加载中状态（内容区显示骨架屏）
    let w0 = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(u) = w0.upgrade() { u.set_loading(true); }
    });

    // ---- 缓存即显：先用上次启动拉到的板块数据填充（封面全走磁盘缓存，几乎零等待），
    //      随后网络刷新二次填充覆盖；网络失败时旧缓存兜底，板块不空白 ----
    if let Some(c) = load_home_cache() {
        apply_home_data(weak.clone(), &cfg, &c.tracks, &c.top_cover, &c.rp, &c.ta, &c.na, &c.ns, &c.daily, true).await;
    }
    // 并行两组：① 热榜 → 榜单详情 tracks（含多榜单兜底）② 其余四个板块 API。
    // tracks 链与其它 API 同时进行，避免 toplist→tracks 串行拖慢整体完成时间。
    let (tracks_chain, rp, ta, na, ns, daily_chain) = tokio::join!(
        async {
            let tl = service::toplist(&client).await;
            let hot_id = tl
                .ok()
                .and_then(|v| v.pointer("/data").and_then(|a| a.as_array()).cloned())
                .and_then(|arr| arr.first().and_then(|f| f.get("id").and_then(|i| i.as_u64())))
                .unwrap_or(19723756); // 云音乐飙升榜
            let (mut tracks, mut cover) = toptracks::tl_det_tracks(&client, hot_id).await;
            // 榜单详情偶返回空 tracks：依次尝试多个榜单
            if tracks.is_empty() {
                for fallback_id in [19723756u64, 3778678, 3779629, 2884035, 2256276] {
                    if fallback_id == hot_id { continue; }
                    let (t, c) = toptracks::tl_det_tracks(&client, fallback_id).await;
                    if !t.is_empty() { tracks = t; cover = c; break; }
                }
            }
            (tracks, cover)
        },
        async { service::recommend_playlist(&client, 10).await },
        async { service::top_artists(&client, 24, 0).await },
        async { service::new_albums(&client, "ALL", 5, 0).await },
        async { service::new_songs(&client, 30).await },
        async { service::recommend_songs(&client).await },
    );
    // 注意：不再用新歌速递数据顶替 tracks——保证新歌速递与 Hero 每日精选数据源独立。
    // 榜单失败时 tracks 为空，最热/队列显示为空，新歌速递仍正常显示自己的接口数据。
    let (tracks, top_cover) = tracks_chain;
    let daily = daily_chain.ok()
        .and_then(|v| v.pointer("/data/dailySongs").and_then(|t| t.as_array()).cloned())
        .unwrap_or_default();
    let (rp, ta, na, ns) = (
        rp.ok().unwrap_or_default(), ta.ok().unwrap_or_default(),
        na.ok().unwrap_or_default(), ns.ok().unwrap_or_default(),
    );

    // 网络刷新填充（restore_enable=false：不覆盖当前曲目，避免刷新重置用户已切歌）
    apply_home_data(weak.clone(), &cfg, &tracks, &top_cover, &rp, &ta, &na, &ns, &daily, false).await;

    // 拉取成功（榜单 tracks 非空）才回写缓存；失败保留旧缓存兜底下次启动
    if !tracks.is_empty() {
        save_home_cache(&HomeCache {
            ts: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs()).unwrap_or(0),
            tracks: tracks.clone(),
            top_cover: top_cover.clone(),
            rp, ta, na, ns,
            daily: daily.clone(),
        });
    }
}

// ---- 首页板块数据缓存（stale-while-revalidate）----
// 启动先读上次拉取的板块 JSON 立即填充（封面全走磁盘缓存，几乎零等待），
// 随后网络刷新成功二次填充并回写；网络失败时旧缓存兜底。
// 存解析后的中间结构（榜单歌曲数组 + 各板块响应），填充链路复用 apply_home_data。
#[derive(serde::Serialize, serde::Deserialize)]
struct HomeCache {
    ts: u64,
    tracks: Vec<serde_json::Value>,
    top_cover: String,
    rp: serde_json::Value,
    ta: serde_json::Value,
    na: serde_json::Value,
    ns: serde_json::Value,
    daily: Vec<serde_json::Value>,
}

fn home_cache_path() -> std::path::PathBuf {
    crate::persist::config::cache_dir().join("discovery").join("home.json")
}

fn load_home_cache() -> Option<HomeCache> {
    serde_json::from_slice(&std::fs::read(home_cache_path()).ok()?).ok()
}

fn save_home_cache(c: &HomeCache) {
    let p = home_cache_path();
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    // 临时文件改名写入，避免写一半崩溃损坏缓存
    let tmp = p.with_extension("json.tmp");
    if let Ok(b) = serde_json::to_vec(c) {
        if std::fs::write(&tmp, &b).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// 把板块数据（原始 JSON）解析并填充到首页模型（第一批文本 + 第二批封面补图）。
/// restore_enable=true 时执行启动恢复（队列匹配 last_song 设当前曲），网络刷新调用传 false。
#[allow(clippy::too_many_arguments)]
async fn apply_home_data(
    weak: slint::Weak<crate::AppWindow>,
    cfg: &AppConfig,
    tracks: &[serde_json::Value],
    top_cover: &str,
    rp: &serde_json::Value,
    ta: &serde_json::Value,
    na: &serde_json::Value,
    ns: &serde_json::Value,
    daily: &[serde_json::Value],
    restore_enable: bool,
) {
    // Hero 数据源对齐 old（GET /recommend/songs → 网易 v3/discovery/recommend/songs）：
    // 登录态取每日推荐 data.dailySongs（30 首），未登录/失败（code=301）回落榜单 tracks。
    let (hero_tracks, hero_cover_url) = if daily.is_empty() {
        (tracks.to_vec(), top_cover.to_string())
    } else {
        // 每日推荐无榜单封面：用第一首歌的专辑封面作 Hero/详情页背景兜底
        let c = daily.first().map(|s| song_cover_url(s)).unwrap_or_default();
        (daily.to_vec(), c)
    };
    // Hero 卡/详情页头部背景 URL（记忆到全局，详情页共用）
    *HERO_COVER_URL.lock().unwrap() = hero_cover_url.clone();

    // Hero 歌曲池：每日推荐全量（未登录回落榜单 tracks；播放全部/详情页数据源）
    {
        let pool: Vec<SongBase> = hero_tracks.iter().map(|s| {
            let (id, name, artist, album, dt) = song_fields(s);
            SongBase { id, name, artist, album, duration: fmt_dur(dt), duration_sec: dt as f32, url: song_cover_url(s) }
        }).collect();
        *HERO_POOL.lock().unwrap() = Arc::new(pool);
    }

    // ---- 收集各板块基础数据（不含 cover）+ 封面 URL 清单 ----
    let hot: Vec<HotBase> = tracks.iter().enumerate().take(12)
        .map(|(i, s)| {
            let (id, name, artist, _, _) = song_fields(s);
            HotBase { id, rank: (i + 1).to_string().into(), name, artist, from: from(i), to: to(i), url: song_cover_url(s) }
        })
        .collect();

    let hero: Vec<HeroSong> = hero_tracks.iter().enumerate().take(5)
        .map(|(i, s)| {
            let (id, name, artist, _, _) = song_fields(s);
            HeroSong { id, rank: (i + 1).to_string().into(), name, artist }
        })
        .collect();

    let queue: Vec<SongBase> = tracks.iter().take(20)
        .map(|s| {
            let (id, name, artist, album, dt) = song_fields(s);
            SongBase { id, name, artist, album, duration: fmt_dur(dt), duration_sec: dt as f32, url: song_cover_url(s) }
        })
        .collect();

    let playlists = playlists::build_playlists(&Ok(rp.clone()));
    let artists = artists::build_artists(&Ok(ta.clone()));
    let albums = albums::build_albums(&Ok(na.clone()));
    let newsongs = newsongs::build_newsongs(&Ok(ns.clone()));

    // 当前曲目恢复目标：优先上次播放歌曲（队列中匹配），否则队列第一首。
    // 封面分两批：第一批全部占位（不阻塞 UI 线程），第二批批量取图（磁盘缓存优先）后补图。
    // 网络刷新调用（restore_enable=false）跳过恢复，避免覆盖用户已切换的歌曲。
    let last_id = cfg.last_song.id as i32;
    let restore = if restore_enable {
        queue.iter().find(|q| last_id != 0 && q.id == last_id)
            .cloned()
            .or_else(|| queue.first().cloned())
    } else {
        None
    };

    // 新歌速递：单一全量模型（不再 step_by(2) 拆左右两列重复拉取），UI 按 3~6 列网格排

    // ---- 第一批填充：文本数据 + 队列 + 当前曲目（封面一律占位）----
    // 页面立即可见（骨架屏关闭），封面随后由第二批补图。
    // 注意：绝不能在 UI 线程同步解码上百张封面 —— debug 构建下单张 PNG 解码可达百毫秒级，
    // 会阻塞首帧 + data-ready 数十秒（表现为启动后窗口白屏"未响应"）。
    // 注意：slint::Image 非线程安全 —— 像素缓冲可在后台线程解码，Image 构造仍须在 UI 线程。
    let current_first = restore.clone();
    // 第一批闭包使用克隆份数据，原份数据留给第二批补图时重建模型
    let hot1 = hot.clone(); let hero1 = hero.clone();
    let pl1 = playlists.clone(); let ar1 = artists.clone(); let al1 = albums.clone();
    let ns1 = newsongs.clone();
    let q1 = queue.clone();
    let hero_count1 = hero_tracks.len();
    let weak1 = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak1.upgrade() else { return; };
        ui.set_hot_songs(model(hot1.iter().map(|d| HotSong {
            id: d.id, rank: d.rank.clone(), name: d.name.clone(), artist: d.artist.clone(),
            cover: Image::default(),
            from: d.from, to: d.to,
        }).collect::<Vec<_>>()));
        ui.set_hero_songs(model(hero1.clone()));
        // Hero 卡"每日推荐 N 首"真实数量
        ui.set_hero_count(hero_count1 as i32);
        // Hero/详情页头部背景：先占位，磁盘缓存解码在后台线程完成后补图
        // （原图解码实测 0.75~1.7s，同步解码会卡住本闭包拖慢骨架屏关闭）
        ui.set_hero_cover(Image::default());

        // 首页歌单网格（单模型，流式换行）
        ui.set_recommend_playlists(model(pl1.iter().map(|d| Playlist {
            id: d.id, name: d.name.clone(), plays: d.plays.clone(),
            cover: Image::default(),
            from: d.from, to: d.to,
        }).collect::<Vec<_>>()));
        // 歌单页全部歌单（独立模型）
        ui.set_playlists_all(model(pl1.iter().map(|d| Playlist {
            id: d.id, name: d.name.clone(), plays: d.plays.clone(),
            cover: Image::default(),
            from: d.from, to: d.to,
        }).collect::<Vec<_>>()));

        ui.set_artists(model(ar1.iter().map(|d| Artist {
            id: d.id, name: d.name.clone(), initial: d.initial.clone(),
            cover: Image::default(),
            from: d.from, to: d.to,
        }).collect::<Vec<_>>()));

        ui.set_albums(model(al1.iter().map(|d| Album {
            id: d.id, name: d.name.clone(), artist: d.artist.clone(), year: d.year.clone(),
            cover: Image::default(),
            from: d.from, to: d.to,
        }).collect::<Vec<_>>()));

        ui.set_new_songs(model(ns1.iter().map(|d| NewSong {
            id: d.id, name: d.name.clone(), artist: d.artist.clone(),
            cover: Image::default(),
        }).collect::<Vec<_>>()));

        let songs: Vec<Song> = q1.iter().map(|d| Song {
            id: d.id, name: d.name.clone(), artist: d.artist.clone(), album: d.album.clone(),
            cover: Image::default(),
            duration: d.duration.clone(), duration_sec: d.duration_sec,
            url: d.url.clone().into(),
            liked: crate::persist::liked::is_liked(d.id as u32 as i64),
        }).collect();
        if !songs.is_empty() {
            ui.set_queue(model(songs));
        }
        if let Some(r) = current_first {
            // 启动恢复：按收藏集合同步当前歌曲爱心状态（不自动播放）；
            // 封面先占位，后台线程磁盘缓存解码完成后补图（id 防护）
            let song = Song {
                id: r.id, name: r.name.clone(), artist: r.artist.clone(), album: r.album.clone(),
                cover: Image::default(),
                duration: r.duration.clone(), duration_sec: r.duration_sec,
                url: r.url.into(),
                liked: crate::persist::liked::is_liked(r.id as u32 as i64),
            };
            let liked = crate::persist::liked::is_liked(song.id as u32 as i64);
            ui.set_current_song(song);
            ui.set_liked(liked);
        }
        // 第一批完成：关闭骨架屏（页面立即可见，封面随后补图）
        ui.set_loading(false);
        log::info!("[boot] data-ready {}ms", crate::boot_elapsed_ms());
    });

    // ---- Hero 背景 + 当前曲封面：统一走 cover_warm（后台解码 + UI 线程回调）----
    {
        let hero_url_bg = hero_cover_url.clone();
        let hero_weak = weak.clone();
        cover_warm(&hero_url_bg, CoverType::Song, move |img| {
            if let Some(u) = hero_weak.upgrade() {
                u.set_hero_cover(img);
            }
        });
        if let Some(r) = restore.as_ref() {
            let cur_url = r.url.clone();
            let cur_id = r.id;
            let cur_weak = weak.clone();
            cover_warm(&cur_url, CoverType::Song, move |img| {
                if let Some(u) = cur_weak.upgrade() {
                    // id 防护：解码期间用户已切歌则丢弃，避免覆盖新歌封面
                    if u.get_current_song().id == cur_id {
                        let mut s = u.get_current_song();
                        s.cover = img;
                        u.set_current_song(s);
                    }
                }
            });
        }
    }

    // ---- 第二批：并行获取全部封面（磁盘缓存优先，未命中下载写盘）并补图 ----
    let mut urls: Vec<(String, CoverType)> = Vec::new();
    if !hero_cover_url.is_empty() { urls.push((hero_cover_url.clone(), CoverType::Song)); }
    for h in &hot { urls.push((h.url.clone(), CoverType::Song)); }
    for p in &playlists { urls.push((p.url.clone(), CoverType::Playlist)); }
    for a in &artists { urls.push((a.url.clone(), CoverType::Artist)); }
    for al in &albums { urls.push((al.url.clone(), CoverType::Album)); }
    for n in &newsongs { urls.push((n.url.clone(), CoverType::Song)); }
    for q in &queue { urls.push((q.url.clone(), CoverType::Song)); }
    let covers = fetch_covers(&urls).await;

    // 当前曲目补图目标：仍是启动恢复的那首才覆盖（避免覆盖用户已切换的歌曲）
    let restore_id = restore.as_ref().map(|r| r.id).unwrap_or(0);
    let restore_url = restore.as_ref().map(|r| r.url.clone()).unwrap_or_default();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return; };
        // Hero/详情页头部背景补图
        if !hero_cover_url.is_empty() {
            ui.set_hero_cover(cov(&covers, &hero_cover_url));
        }
        let hot_songs: Vec<HotSong> = hot.into_iter().map(|d| HotSong {
            id: d.id, rank: d.rank, name: d.name, artist: d.artist,
            cover: cov(&covers, &d.url),
            from: d.from, to: d.to,
        }).collect();
        ui.set_hot_songs(model(hot_songs));

        let rpl: Vec<Playlist> = playlists.iter().map(|d| Playlist {
            id: d.id, name: d.name.clone(), plays: d.plays.clone(),
            cover: cov(&covers, &d.url),
            from: d.from, to: d.to,
        }).collect();
        ui.set_recommend_playlists(model(rpl));
        // 歌单页全部歌单（独立模型）
        let playlists_all: Vec<Playlist> = playlists.iter().map(|d| Playlist {
            id: d.id, name: d.name.clone(), plays: d.plays.clone(),
            cover: cov(&covers, &d.url),
            from: d.from, to: d.to,
        }).collect();
        ui.set_playlists_all(model(playlists_all));

        let arts: Vec<Artist> = artists.into_iter().map(|d| Artist {
            id: d.id, name: d.name, initial: d.initial,
            cover: cov(&covers, &d.url),
            from: d.from, to: d.to,
        }).collect();
        ui.set_artists(model(arts));

        let albs: Vec<Album> = albums.into_iter().map(|d| Album {
            id: d.id, name: d.name, artist: d.artist, year: d.year,
            cover: cov(&covers, &d.url),
            from: d.from, to: d.to,
        }).collect();
        ui.set_albums(model(albs));

        let ns: Vec<NewSong> = newsongs.into_iter().map(|d| NewSong {
            id: d.id, name: d.name, artist: d.artist,
            cover: cov(&covers, &d.url),
        }).collect();
        ui.set_new_songs(model(ns));

        // 队列逐行补图（更新行数据，保持模型与索引稳定）
        let q = ui.get_queue();
        for i in 0..q.row_count() {
            let Some(s) = q.row_data(i) else { continue; };
            let url = s.url.to_string();
            if url.is_empty() { continue; }
            if let Some(buf) = covers.get(&url).and_then(|x| x.as_ref()) {
                let mut ns = s.clone();
                ns.cover = slint::Image::from_rgba8(buf.clone());
                q.set_row_data(i, ns);
            }
        }

        // 当前曲目封面补图
        if restore_id != 0 && ui.get_current_song().id == restore_id && !restore_url.is_empty() {
            let mut cur = ui.get_current_song();
            cur.cover = cov(&covers, &restore_url);
            ui.set_current_song(cur);
        }
    });
}
