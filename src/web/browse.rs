//! web 浏览数据：首页 / 歌单 / 详情 / 我的最爱 / 最近播放 / 电台 / MV / 专辑 页面数据流。
//!
//! 归属回调（appwindow.slint root，唯一接线方，勿与其他模块重复）：
//!   open-daily-detail / open-resource-detail(int,int) / detail-back / detail-search-changed(string) /
//!   detail-like-toggle / playlist-cat-clicked(int) / album-cat-clicked(int) / radio-cat-clicked(int) /
//!   liked-unlike(int,int) / recent-remove(int) / recent-clear() / page-fill(int) / nav-anim
//!
//! 数据通道（gdmusic 能力面已探测确认，勿假设存在未确认端点）：
//!   gd::playlist_detail(id, limit) —— 歌单详情（name/cover/description/tracks[]，截断前 limit 首）
//!   gd::search(q, count, pages)    —— 歌曲搜索（首页 hero / 歌手 / 专辑 / 新歌 派生数据）
//!   gd::register_full / lookup_full / liked_* / recent_* —— ID 注册表与本地收藏/最近播放内存态
//!   gdmusic 无 toplist/recommend/suggest/album/mv/dj 端点（400）→ 相关页面留空态并
//!   用 toast 提示"该板块 Web 端待补充"，不强求与 desktop 同源。
//!
//! 首页推荐歌单固定榜单 id（已探测有效，勿改）：
//!   3778678 热歌榜 / 3779629 新歌榜 / 19723756 飙升榜 / 2250011882 抖音排行榜 / 60198 Billboard 榜
//!   歌单卡数据 = 逐个 gd::playlist_detail(id) 取 name + tracks 数（并发 spawn_local，5 槽位齐后统一回填）。
//!   注意：2250011882 超过 i32::MAX，登记 UI id 时 `as i32` 回绕，并用 PLID_MAP 反查原始字符串 id。
//!
//! 关键约束：
//! - 详情打开（open-resource-detail(kind,id)）：kind=0 歌单 → gd::playlist_detail → 填
//!   detail-title/detail-desc/detail-type/detail-songs（Song 模型，gd::register_full 登记 +
//!   duration 用 GdTrack.duration_sec 格式化 mm:ss）+ detail-all-songs 同值 + detail-visible=true
//!   （先置 fade=false + shift=-40px，20ms 后回正触发滑入淡入动画）。
//! - open-daily-detail：web 用模块缓存的 hero 列表作为详情数据（无每日推荐接口；res-kind=-1 隐藏爱心）。
//! - 打开时 loading=true → 数据回填后 false；失败 toast。
//! - page-fill(nav)：7=我的最爱（gd::liked_all + lookup_full 重建）8=最近播放（gd::recent_list 重建）。
//! - nav-anim：内容区淡入淡出（page-fade 先 false 20ms 再 true）。
//! - 封面统一 gd::cover_image("")（返回空图，UI 走 from/to 渐变兜底；浏览器直连 126 CDN 被阻断）。

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use wasm_bindgen_futures::spawn_local;

use crate::web::gd;
use crate::web::show_toast;

/// 首页推荐榜单（id, 展示名仅用于日志/占位）——已探测有效固定榜单 id。
const PLAYLIST_IDS: &[&str] = &[
    "3778678",      // 热歌榜
    "3779629",      // 新歌榜
    "19723756",     // 飙升榜
    "2250011882",   // 抖音排行榜（>i32::MAX，登记时回绕）
    "60198",        // Billboard 榜
];

/// 渐变配色盘（from, to）——对齐 desktop core::song_util::PALETTE，按索引轮换。
/// web 端不编译 core，此处复制一份。
const PALETTE: &[(u32, u32)] = &[
    (0x7c6bff, 0x22d3ee),
    (0xf59e0b, 0xef4444),
    (0x22c55e, 0x14b8a6),
    (0x8b5cf6, 0xec4899),
    (0x0ea5e9, 0x6366f1),
    (0xf472b6, 0xa78bfa),
    (0xfcd34d, 0xf59e0b),
    (0xa5b4fc, 0x818cf8),
    (0xf0abfc, 0xe879f9),
    (0x34d399, 0x0ea5e9),
    (0xfed7aa, 0xfb923c),
    (0xa7f3d0, 0x34d399),
];

fn grad(i: usize) -> (slint::Color, slint::Color) {
    let (f, t) = PALETTE[i % PALETTE.len()];
    (
        slint::Color::from_argb_u8(0xFF, ((f >> 16) & 0xFF) as u8, ((f >> 8) & 0xFF) as u8, (f & 0xFF) as u8),
        slint::Color::from_argb_u8(0xFF, ((t >> 16) & 0xFF) as u8, ((t >> 8) & 0xFF) as u8, (t & 0xFF) as u8),
    )
}

/// 秒 → "mm:ss"（Song.duration 展示串）。
fn fmt_dur(sec: f32) -> String {
    let s = sec.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Hero 缓存条目（首页搜索结果登记后的 UI id + 元数据，供 open-daily-detail 重建详情）。
#[derive(Clone)]
struct HeroEntry {
    ui_id: i32,
    name: String,
    artist: String,
    album: String,
    duration_sec: f32,
}

thread_local! {
    /// 5 榜单并发拉取槽位（与 PLAYLIST_IDS 同序）；全部 Some 后统一回填 recommend-playlists。
    static REC_SLOTS: RefCell<Vec<Option<crate::Playlist>>> = RefCell::new((0..PLAYLIST_IDS.len()).map(|_| None).collect());
    /// 与 REC_SLOTS 同序的 cover_url（GdPlaylist.coverImgUrl），槽位齐后异步拉封面用。
    static REC_COVERS: RefCell<Vec<Option<String>>> = RefCell::new((0..PLAYLIST_IDS.len()).map(|_| None).collect());
    /// 榜单模型缓存：歌单页 playlists-all 复用，避免切分类重复请求。
    static PLAYLISTS_CACHE: RefCell<Vec<crate::Playlist>> = RefCell::new(Vec::new());
    /// Hero（首页"热门"搜索）缓存，供每日精选详情打开时复用。
    static HERO_CACHE: RefCell<Vec<HeroEntry>> = RefCell::new(Vec::new());
    /// UI 歌单 id（i32，可能因溢出回绕）→ gdmusic 原始字符串 id 反查表。
    static PLID_MAP: RefCell<Vec<(i32, String)>> = RefCell::new(Vec::new());
}

/// 详情覆盖层打开动画：先淡出 + 左移，20ms 后回正触发滑入淡入。
fn open_detail_anim(u: &crate::AppWindow) {
    u.set_detail_fade(false);
    u.set_detail_shift(-40.0);
    u.set_detail_visible(true);
    let w = u.as_weak();
    slint::Timer::single_shot(Duration::from_millis(20), move || {
        if let Some(x) = w.upgrade() {
            x.set_detail_fade(true);
            x.set_detail_shift(0.0);
        }
    });
}

/// Hero 缓存条目 → Song 模型（每日精选详情）。
fn entries_to_songs(entries: &[HeroEntry]) -> Vec<crate::Song> {
    entries
        .iter()
        .map(|e| crate::Song {
            id: e.ui_id,
            name: e.name.clone().into(),
            artist: e.artist.clone().into(),
            album: e.album.clone().into(),
            cover: gd::cover_image(""),
            duration: fmt_dur(e.duration_sec).into(),
            duration_sec: e.duration_sec,
            url: String::new().into(),
            liked: gd::liked_contains(e.ui_id),
        })
        .collect()
}

/// 每日精选详情回填（title/type/res-kind=-1 隐藏爱心）+ 打开动画。
fn open_daily_into(u: &crate::AppWindow, entries: Vec<HeroEntry>) {
    let songs = entries_to_songs(&entries);
    u.set_detail_title("每日为你精选".into());
    u.set_detail_desc("基于当前热门歌曲为你精选".into());
    u.set_detail_type("每日推荐".into());
    u.set_detail_res_kind(-1);
    u.set_detail_res_id(0);
    u.set_detail_liked(false);
    u.set_detail_search("".into());
    // 每日推荐详情大封面：复用首页 Hero 卡封面背景（已异步填充，未加载/失败为渐变）
    u.set_detail_cover(u.get_hero_cover());
    let model: ModelRc<crate::Song> = Rc::new(VecModel::from(songs)).into();
    u.set_detail_all_songs(model.clone());
    u.set_detail_songs(model);
    open_detail_anim(u);
}

/// 5 榜单槽位是否齐；齐则回填 recommend-playlists / playlists-all（缓存复用）+ 关 loading。
fn try_flush_recommend(ui: &slint::Weak<crate::AppWindow>) {
    let ready = REC_SLOTS.with(|s| s.borrow().iter().all(|x| x.is_some()));
    if !ready {
        return;
    }
    let v: Vec<crate::Playlist> =
        REC_SLOTS.with(|s| s.borrow().iter().map(|x| x.clone().unwrap()).collect());
    PLAYLISTS_CACHE.with(|c| *c.borrow_mut() = v.clone());
    if let Some(u) = ui.upgrade() {
        u.set_recommend_playlists(Rc::new(VecModel::from(v.clone())).into());
        u.set_playlists_all(Rc::new(VecModel::from(v)).into());
        u.set_loading(false);
        // 异步封面：两模型同序同图，按 cover_url 拉取；失败静默保持渐变。
        let covers: Vec<String> =
            REC_COVERS.with(|s| s.borrow().iter().map(|x| x.clone().unwrap_or_default()).collect());
        for mdl in [u.get_recommend_playlists(), u.get_playlists_all()] {
            for (i, cu) in covers.iter().enumerate().take(30) {
                if cu.is_empty() {
                    continue;
                }
                let mm = mdl.clone();
                let cu = cu.clone();
                gd::spawn_cover_url(cu, move |img| {
                    if let Some(img) = img {
                        if let Some(mut row) = mm.row_data(i) {
                            row.cover = img.clone();
                            mm.set_row_data(i, row);
                        }
                    }
                });
            }
        }
    }
}

/// 我的最爱页重建：gd::liked_all() + lookup_full → Song 模型（liked=true）。
fn rebuild_liked(ui: &slint::Weak<crate::AppWindow>) {
    let ids = gd::liked_all();
    let v: Vec<crate::Song> = ids
        .iter()
        .filter_map(|id| {
            gd::lookup_full(*id).map(|(_src, _gid, name, artist, album, dur)| crate::Song {
                id: *id,
                name: name.into(),
                artist: artist.into(),
                album: album.into(),
                cover: gd::cover_image(""),
                duration: fmt_dur(dur).into(),
                duration_sec: dur,
                url: String::new().into(),
                liked: true,
            })
        })
        .collect();
    if let Some(u) = ui.upgrade() {
        u.set_liked_song_list(Rc::new(VecModel::from(v)).into());
    }
}

/// 最近播放页重建：gd::recent_list() → Song 模型（duration 经 lookup_full 补全）。
fn rebuild_recent(ui: &slint::Weak<crate::AppWindow>) {
    let list = gd::recent_list();
    let v: Vec<crate::Song> = list
        .into_iter()
        .map(|(id, name, artist, album)| {
            let dur = gd::lookup_full(id).map(|(_, _, _, _, _, d)| d).unwrap_or(0.0);
            crate::Song {
                id,
                name: name.into(),
                artist: artist.into(),
                album: album.into(),
                cover: gd::cover_image(""),
                duration: fmt_dur(dur).into(),
                duration_sec: dur,
                url: String::new().into(),
                liked: gd::liked_contains(id),
            }
        })
        .collect();
    if let Some(u) = ui.upgrade() {
        u.set_recent_song_list(Rc::new(VecModel::from(v)).into());
    }
}

pub fn setup(ui: &crate::AppWindow) {
    // ---- 启动：loading 占位 + 歌单分类（本地常量，无需请求）----
    ui.set_loading(true);
    let cats: Vec<slint::SharedString> = [
        "每日推荐",
        "热歌榜",
        "新歌榜",
        "飙升榜",
        "抖音榜",
        "Billboard 榜",
    ]
    .iter()
    .map(|s| slint::SharedString::from(*s))
    .collect();
    ui.set_playlist_categories(Rc::new(VecModel::from(cats)).into());

    // ---- 首页推荐歌单：并发拉 5 个固定榜单详情（槽位齐后统一回填）----
    for (i, id) in PLAYLIST_IDS.iter().enumerate() {
        let id: &'static str = *id;
        let ui_w = ui.as_weak();
        spawn_local(async move {
            let pid: i32 = id.parse::<i64>().unwrap_or(0) as i32;
            PLID_MAP.with(|m| m.borrow_mut().push((pid, id.to_string())));
            let (from, to) = grad(i);
            let (item, cover_url) = match gd::playlist_detail(id, 60).await {
                Some(pl) => {
                    let cu = pl.cover_url.clone();
                    (
                        crate::Playlist {
                            id: pid,
                            name: pl.name.into(),
                            plays: format!("{}首", pl.tracks.len()).into(),
                            cover: gd::cover_image(""),
                            from,
                            to,
                        },
                        cu,
                    )
                }
                None => (
                    crate::Playlist {
                        id: pid,
                        name: "加载失败".into(),
                        plays: "".into(),
                        cover: gd::cover_image(""),
                        from,
                        to,
                    },
                    String::new(),
                ),
            };
            REC_SLOTS.with(|s| s.borrow_mut()[i] = Some(item));
            REC_COVERS.with(|s| s.borrow_mut()[i] = Some(cover_url));
            try_flush_recommend(&ui_w);
        });
    }

    // ---- Hero + 热歌榜 + 派生歌手/专辑：gd::search("热门") ----
    {
        let ui_w = ui.as_weak();
        spawn_local(async move {
            let songs = gd::search("热门", 10, 1).await;
            if songs.is_empty() {
                if let Some(u) = ui_w.upgrade() {
                    u.set_loading(false);
                    show_toast(&u, "首页数据加载失败", "error");
                }
                return;
            }
            let mut hero: Vec<crate::HeroSong> = Vec::new();
            let mut hot: Vec<crate::HotSong> = Vec::new();
            let mut artists: Vec<crate::Artist> = Vec::new();
            let mut albums: Vec<crate::Album> = Vec::new();
            let mut hot_pics: Vec<String> = Vec::new();
            let mut artist_pics: Vec<String> = Vec::new();
            let mut album_pics: Vec<String> = Vec::new();
            let mut seen_artists: HashSet<String> = HashSet::new();
            let mut seen_albums: HashSet<String> = HashSet::new();
            let mut cache: Vec<HeroEntry> = Vec::new();
            for (idx, s) in songs.iter().enumerate() {
                let ui_id = gd::register_full("netease", &s.id, &s.name, &s.artist, &s.album, 0.0);
                let rank = (idx + 1).to_string();
                let (from, to) = grad(idx);
                hero.push(crate::HeroSong {
                    id: ui_id,
                    rank: rank.clone().into(),
                    name: s.name.clone().into(),
                    artist: s.artist.clone().into(),
                });
                hot.push(crate::HotSong {
                    id: ui_id,
                    rank: rank.into(),
                    name: s.name.clone().into(),
                    artist: s.artist.clone().into(),
                    cover: gd::cover_image(""),
                    from,
                    to,
                });
                hot_pics.push(s.pic_id.clone());
                if !s.artist.is_empty() && seen_artists.insert(s.artist.clone()) {
                    let initial = s.artist.chars().next().map(|c| c.to_string()).unwrap_or_default();
                    let (af, at) = grad(seen_artists.len() - 1);
                    artists.push(crate::Artist {
                        id: ui_id,
                        name: s.artist.clone().into(),
                        initial: initial.into(),
                        cover: gd::cover_image(""),
                        from: af,
                        to: at,
                    });
                    artist_pics.push(s.pic_id.clone());
                }
                if !s.album.is_empty() && seen_albums.insert(s.album.clone()) {
                    let (af, at) = grad(seen_albums.len() - 1);
                    albums.push(crate::Album {
                        id: ui_id,
                        name: s.album.clone().into(),
                        artist: s.artist.clone().into(),
                        year: "".into(),
                        cover: gd::cover_image(""),
                        from: af,
                        to: at,
                    });
                    album_pics.push(s.pic_id.clone());
                }
                cache.push(HeroEntry {
                    ui_id,
                    name: s.name.clone(),
                    artist: s.artist.clone(),
                    album: s.album.clone(),
                    duration_sec: 0.0,
                });
            }
            HERO_CACHE.with(|c| *c.borrow_mut() = cache);
            if let Some(u) = ui_w.upgrade() {
                u.set_hero_count(hero.len() as i32);
                u.set_hero_songs(Rc::new(VecModel::from(hero)).into());
                u.set_hot_songs(Rc::new(VecModel::from(hot)).into());
                u.set_artists(Rc::new(VecModel::from(artists)).into());
                u.set_albums(Rc::new(VecModel::from(albums)).into());
                // Hero 卡封面背景：热门第一首 pic_id 异步填充（失败保持渐变兜底）
                if let Some(fp) = hot_pics.first() {
                    if !fp.is_empty() {
                        let w = ui_w.clone();
                        let p = fp.clone();
                        spawn_local(async move {
                            if let Some(img) = gd::fetch_cover_pic(&p).await {
                                if let Some(u) = w.upgrade() {
                                    u.set_hero_cover(img);
                                }
                            }
                        });
                    }
                }
                // 异步封面：pic_id → types=pic → 126 图床；空 pic_id 跳过，失败保持渐变。
                let hm = u.get_hot_songs();
                for (i, p) in hot_pics.iter().enumerate().take(30) {
                    if p.is_empty() { continue; }
                    let mm = hm.clone();
                    let p = p.clone();
                    gd::spawn_cover_pic(p, move |img| {
                        if let Some(img) = img {
                            if let Some(mut row) = mm.row_data(i) {
                                row.cover = img.clone();
                                mm.set_row_data(i, row);
                            }
                        }
                    });
                }
                let am = u.get_artists();
                for (i, p) in artist_pics.iter().enumerate().take(30) {
                    if p.is_empty() { continue; }
                    let mm = am.clone();
                    let p = p.clone();
                    gd::spawn_cover_pic(p, move |img| {
                        if let Some(img) = img {
                            if let Some(mut row) = mm.row_data(i) {
                                row.cover = img.clone();
                                mm.set_row_data(i, row);
                            }
                        }
                    });
                }
                let alm = u.get_albums();
                for (i, p) in album_pics.iter().enumerate().take(30) {
                    if p.is_empty() { continue; }
                    let mm = alm.clone();
                    let p = p.clone();
                    gd::spawn_cover_pic(p, move |img| {
                        if let Some(img) = img {
                            if let Some(mut row) = mm.row_data(i) {
                                row.cover = img.clone();
                                mm.set_row_data(i, row);
                            }
                        }
                    });
                }
            }
        });
    }

    // ---- 新歌速递：gd::search("新歌") ----
    {
        let ui_w = ui.as_weak();
        spawn_local(async move {
            let songs = gd::search("新歌", 10, 1).await;
            let pics: Vec<String> = songs.iter().map(|s| s.pic_id.clone()).collect();
            let v: Vec<crate::NewSong> = songs
                .iter()
                .map(|s| {
                    let ui_id = gd::register_full("netease", &s.id, &s.name, &s.artist, &s.album, 0.0);
                    crate::NewSong {
                        id: ui_id,
                        name: s.name.clone().into(),
                        artist: s.artist.clone().into(),
                        cover: gd::cover_image(""),
                    }
                })
                .collect();
            if let Some(u) = ui_w.upgrade() {
                u.set_new_songs(Rc::new(VecModel::from(v)).into());
                let nm = u.get_new_songs();
                for (i, p) in pics.iter().enumerate().take(30) {
                    if p.is_empty() { continue; }
                    let mm = nm.clone();
                    let p = p.clone();
                    gd::spawn_cover_pic(p, move |img| {
                        if let Some(img) = img {
                            if let Some(mut row) = mm.row_data(i) {
                                row.cover = img.clone();
                                mm.set_row_data(i, row);
                            }
                        }
                    });
                }
            }
        });
    }

    // ---- open-resource-detail(kind, id)：kind=0 歌单拉详情；其余板块 Web 端待补充 ----
    {
        let ui_w = ui.as_weak();
        ui.on_open_resource_detail(move |kind, id| {
            if kind != 0 {
                if let Some(u) = ui_w.upgrade() {
                    show_toast(&u, "该板块 Web 端待补充", "info");
                }
                return;
            }
            // id 可能因 >i32::MAX 回绕：优先反查 PLID_MAP 拿原始字符串 id
            let orig = PLID_MAP
                .with(|m| m.borrow().iter().find(|(i, _)| *i == id).map(|(_, s)| s.clone()))
                .unwrap_or_else(|| id.to_string());
            let ui_w2 = ui_w.clone();
            spawn_local(async move {
                if let Some(u) = ui_w2.upgrade() {
                    u.set_loading(true);
                }
                match gd::playlist_detail(&orig, 100).await {
                    Some(pl) => {
                        let songs: Vec<crate::Song> = pl
                            .tracks
                            .iter()
                            .map(|t| {
                                let ui_id = gd::register_full(
                                    "netease", &t.id, &t.name, &t.artist, &t.album, t.duration_sec,
                                );
                                crate::Song {
                                    id: ui_id,
                                    name: t.name.clone().into(),
                                    artist: t.artist.clone().into(),
                                    album: t.album.clone().into(),
                                    cover: gd::cover_image(""),
                                    duration: fmt_dur(t.duration_sec).into(),
                                    duration_sec: t.duration_sec,
                                    url: t.cover_url.clone().into(),
                                    liked: gd::liked_contains(ui_id),
                                }
                            })
                            .collect();
                        if let Some(u) = ui_w2.upgrade() {
                            u.set_detail_title(pl.name.into());
                            u.set_detail_desc(pl.description.into());
                            u.set_detail_type("歌单".into());
                            u.set_detail_res_kind(0);
                            u.set_detail_res_id(id);
                            u.set_detail_liked(false);
                            u.set_detail_search("".into());
                            let model: ModelRc<crate::Song> = Rc::new(VecModel::from(songs)).into();
                            u.set_detail_all_songs(model.clone());
                            u.set_detail_songs(model);
                            u.set_loading(false);
                            // 异步封面：tracks 的 al.picUrl 直链；失败保持渐变。
                            let tm = u.get_detail_all_songs();
                            for (i, t) in pl.tracks.iter().enumerate().take(30) {
                                let cu = t.cover_url.clone();
                                if cu.is_empty() {
                                    continue;
                                }
                                let mm = tm.clone();
                                gd::spawn_cover_url(cu, move |img| {
                                    if let Some(img) = img {
                                        if let Some(mut row) = mm.row_data(i) {
                                            row.cover = img.clone();
                                            mm.set_row_data(i, row);
                                        }
                                    }
                                });
                            }
                            // 详情大封面：歌单 cover_url 异步填充（失败保持渐变兜底）
                            if !pl.cover_url.is_empty() {
                                let w3 = ui_w2.clone();
                                let cu = pl.cover_url.clone();
                                spawn_local(async move {
                                    if let Some(img) = gd::fetch_cover_url(&cu).await {
                                        if let Some(u) = w3.upgrade() {
                                            u.set_detail_cover(img);
                                        }
                                    }
                                });
                            }
                            open_detail_anim(&u);
                        }
                    }
                    None => {
                        if let Some(u) = ui_w2.upgrade() {
                            u.set_loading(false);
                            show_toast(&u, "歌单加载失败", "error");
                        }
                    }
                }
            });
        });
    }

    // ---- open-daily-detail：用 Hero 缓存构建每日精选详情（缓存空则现拉一次）----
    {
        let ui_w = ui.as_weak();
        ui.on_open_daily_detail(move || {
            let entries = HERO_CACHE.with(|c| c.borrow().clone());
            if !entries.is_empty() {
                if let Some(u) = ui_w.upgrade() {
                    open_daily_into(&u, entries);
                }
                return;
            }
            let w = ui_w.clone();
            spawn_local(async move {
                let songs = gd::search("热门", 10, 1).await;
                let entries: Vec<HeroEntry> = songs
                    .iter()
                    .map(|s| {
                        let ui_id = gd::register_full("netease", &s.id, &s.name, &s.artist, &s.album, 0.0);
                        HeroEntry {
                            ui_id,
                            name: s.name.clone(),
                            artist: s.artist.clone(),
                            album: s.album.clone(),
                            duration_sec: 0.0,
                        }
                    })
                    .collect();
                HERO_CACHE.with(|c| *c.borrow_mut() = entries.clone());
                if let Some(u) = w.upgrade() {
                    open_daily_into(&u, entries);
                }
            });
        });
    }

    // ---- detail-back：淡出 + 左移，20ms 后卸载覆盖层 ----
    {
        let ui_w = ui.as_weak();
        ui.on_detail_back(move || {
            if let Some(u) = ui_w.upgrade() {
                u.set_detail_fade(false);
                u.set_detail_shift(-40.0);
                let w = ui_w.clone();
                slint::Timer::single_shot(Duration::from_millis(20), move || {
                    if let Some(x) = w.upgrade() {
                        x.set_detail_visible(false);
                        x.set_detail_shift(0.0);
                    }
                });
            }
        });
    }

    // ---- detail-search-changed：对 detail-all-songs 按 name/artist 子串过滤 ----
    {
        let ui_w = ui.as_weak();
        ui.on_detail_search_changed(move |t| {
            if let Some(u) = ui_w.upgrade() {
                let all = u.get_detail_all_songs();
                let q = t.to_string().to_lowercase();
                let mut filtered: Vec<crate::Song> = Vec::new();
                for i in 0..all.row_count() {
                    if let Some(s) = all.row_data(i) {
                        if q.is_empty()
                            || s.name.to_string().to_lowercase().contains(q.as_str())
                            || s.artist.to_string().to_lowercase().contains(q.as_str())
                        {
                            filtered.push(s);
                        }
                    }
                }
                u.set_detail_songs(Rc::new(VecModel::from(filtered)).into());
            }
        });
    }

    // ---- detail-like-toggle：翻转详情整体收藏态（内存）----
    {
        let ui_w = ui.as_weak();
        ui.on_detail_like_toggle(move || {
            if let Some(u) = ui_w.upgrade() {
                u.set_detail_liked(!u.get_detail_liked());
            }
        });
    }

    // ---- playlist-cat-clicked：idx=0 恢复缓存榜单；idx 1-5 加载对应固定榜单 ----
    {
        let ui_w = ui.as_weak();
        ui.on_playlist_cat_clicked(move |idx| {
            if let Some(u) = ui_w.upgrade() {
                u.set_playlist_cat(idx);
                if idx == 0 {
                    let v = PLAYLISTS_CACHE.with(|c| c.borrow().clone());
                    if !v.is_empty() {
                        u.set_playlists_all(Rc::new(VecModel::from(v)).into());
                    }
                } else if (1..=PLAYLIST_IDS.len() as i32).contains(&idx) {
                    // web 固定榜单：按分类索引加载对应榜单（每日推荐=合并缓存）
                    let pid = PLAYLIST_IDS[(idx - 1) as usize].to_string();
                    let cat = idx - 1;
                    let w = ui_w.clone();
                    spawn_local(async move {
                        match gd::playlist_detail(&pid, 60).await {
                            Some(pl) => {
                                let (from, to) = grad(cat as usize);
                                let item = crate::Playlist {
                                    id: pid.parse::<i64>().unwrap_or(0) as i32,
                                    name: pl.name.into(),
                                    plays: format!("{}首", pl.tracks.len()).into(),
                                    cover: gd::cover_image(""),
                                    from,
                                    to,
                                };
                                if let Some(u) = w.upgrade() {
                                    u.set_playlists_all(
                                        Rc::new(VecModel::from(vec![item.clone()])).into(),
                                    );
                                    let mm = u.get_playlists_all();
                                    let cu = pl.cover_url.clone();
                                    if !cu.is_empty() {
                                        gd::spawn_cover_url(cu, move |img| {
                                            if let (Some(img), Some(mut row)) =
                                                (img, mm.row_data(0))
                                            {
                                                row.cover = img.clone();
                                                mm.set_row_data(0, row);
                                            }
                                        });
                                    }
                                }
                            }
                            None => {
                                if let Some(u) = w.upgrade() {
                                    show_toast(&u, "榜单加载失败", "error");
                                }
                            }
                        }
                    });
                } else {
                    show_toast(&u, "已切换（Web 端列表待补充）", "info");
                }
            }
        });
    }

    // ---- album-cat / radio-cat：Web 端待补充（留空态）----
    {
        let ui_w = ui.as_weak();
        ui.on_album_cat_clicked(move |_| {
            if let Some(u) = ui_w.upgrade() {
                show_toast(&u, "Web 端待补充", "info");
            }
        });
    }
    {
        let ui_w = ui.as_weak();
        ui.on_radio_cat_clicked(move |_| {
            if let Some(u) = ui_w.upgrade() {
                show_toast(&u, "Web 端待补充", "info");
            }
        });
    }

    // ---- page-fill(nav)：7=我的最爱 8=最近播放 按需重建 ----
    {
        let ui_w = ui.as_weak();
        ui.on_page_fill(move |nav| {
            if nav == 7 {
                rebuild_liked(&ui_w);
            } else if nav == 8 {
                rebuild_recent(&ui_w);
            }
        });
    }

    // ---- liked-unlike(kind, id)：歌曲取消收藏由播放器 like-toggle 负责，资源类 Web 端待补充 ----
    {
        let ui_w = ui.as_weak();
        ui.on_liked_unlike(move |_kind, _id| {
            if let Some(u) = ui_w.upgrade() {
                show_toast(&u, "Web 端待补充", "info");
            }
        });
    }

    // ---- recent-remove / recent-clear：改内存态后重建列表 ----
    {
        let ui_w = ui.as_weak();
        ui.on_recent_remove(move |id| {
            gd::recent_remove_ui(id);
            rebuild_recent(&ui_w);
        });
    }
    {
        let ui_w = ui.as_weak();
        ui.on_recent_clear(move || {
            gd::recent_clear();
            rebuild_recent(&ui_w);
        });
    }

    // ---- nav-anim：内容区淡入（先 fade=false，20ms 后 true）----
    {
        let ui_w = ui.as_weak();
        ui.on_nav_anim(move || {
            if let Some(u) = ui_w.upgrade() {
                u.set_page_fade(false);
                let w = ui_w.clone();
                slint::Timer::single_shot(Duration::from_millis(20), move || {
                    if let Some(x) = w.upgrade() {
                        x.set_page_fade(true);
                    }
                });
            }
        });
    }
}
