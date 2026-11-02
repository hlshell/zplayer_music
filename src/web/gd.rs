//! web(wasm32) 数据通道：gdmusic 开放 CORS API 客户端 + 歌曲 ID 注册表。
//!
//! 已探测确认的 gdmusic 端点（2026-10-23，浏览器 fetch 无 CORS，回显任意 Origin）：
//!   search:   types=search&source=netease|joox|tidal&name=<q>&count=<n>&pages=<p>
//!             → 顶层 JSON 数组 [{id,name,artist[],album,pic_id,url_id,lyric_id,source}]
//!   url:      types=url&source=<src>&id=<id>&br=<br> → {"url":"..."}
//!             （br=320 返回 mp3 兼容最好；br=999 返回 flac 体积大、浏览器兼容差）
//!   playlist: types=playlist&source=netease&id=<id> → {"playlist":{...tracks[]}}
//!             （列表参数（cat/name）一律返回 [] —— 无歌单列表能力，只能按 id 拉详情）
//!   lyric:    types=lyric&source=netease&id=<id> → {"lyric":"<lrc>"}
//!   pic:      types=pic&source=netease&id=<pic_id> → {"url":"<https 封面>"}
//! 不存在的端点（探测 400）：toplist / recommend / suggest / hotwords / catlist / top /
//! album / mv / dj / rank / new_album / artist —— 相关页面走空态/待补充或派生数据。
//!
//! 封面展示（2026-10-23 实测定案）：music.126.net 图片 CDN 返回
//! `Access-Control-Allow-Origin: *`（带 Origin 探测 200 image/jpeg），浏览器 fetch 可直读字节；
//! gdmusic `types=pic` 返回该图床 https URL。slint 1.18.1 wasm 无运行时 URL 图片加载，
//! 故本模块提供 `fetch_cover_url/fetch_cover_pic`：fetch 字节 → image crate 解码 →
//! `slint::Image::from_rgba8`（内存缓存去重）。业务模块用 `spawn_cover_url/spawn_cover_pic`
//! 异步填充模型行 cover（失败保持组件渐变兜底，不阻塞数据流）。

use std::cell::RefCell;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// 播放品质：320 返回 mp3（浏览器兼容最好）；999 返回 flac。
pub const DEFAULT_BR: &str = "320";

#[derive(Clone, Debug, Default)]
pub struct GdSong {
    /// gdmusic 原始字符串 id（netease 数字串 / joox base64）
    pub id: String,
    pub name: String,
    /// 首个歌手（UI 单字段展示）
    pub artist: String,
    pub album: String,
    pub pic_id: String,
    pub url_id: String,
    pub lyric_id: String,
    /// netease / joox / tidal
    pub source: String,
}

#[derive(Clone, Debug, Default)]
pub struct GdTrack {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    /// al.picUrl（已 https 化；阶段 1b 展示走渐变兜底）
    pub cover_url: String,
    pub duration_sec: f32,
}

#[derive(Clone, Debug, Default)]
pub struct GdPlaylist {
    pub id: String,
    pub name: String,
    pub cover_url: String,
    pub description: String,
    pub tracks: Vec<GdTrack>,
}

// ---- 歌曲 ID 注册表 -----------------------------------------------------------
// UI Song.id 是 i32（slint callback play-song(int) 的签名），gdmusic id 是任意字符串。
// 填模型时为每首歌分配自增 i32 并登记完整元数据（play.rs 仅凭 UI id 即可重建 current-song）。
thread_local! {
    // (ui_id, source, gd_id, name, artist, album, duration_sec)
    static ID_MAP: RefCell<Vec<(i32, String, String, String, String, String, f32)>> = const { RefCell::new(Vec::new()) };
    static NEXT_UI_ID: RefCell<i32> = const { RefCell::new(1) };
}

/// 登记 (source, gd_id, name, artist, album, duration_sec) 并返回分配的 UI id（自增，永不重复）。
pub fn register_full(source: &str, gd_id: &str, name: &str, artist: &str, album: &str, duration_sec: f32) -> i32 {
    let id = NEXT_UI_ID.with(|n| {
        let mut g = n.borrow_mut();
        let v = *g;
        *g += 1;
        v
    });
    ID_MAP.with(|m| {
        m.borrow_mut().push((
            id,
            source.to_string(),
            gd_id.to_string(),
            name.to_string(),
            artist.to_string(),
            album.to_string(),
            duration_sec,
        ))
    });
    id
}

/// 兼容：仅登记 id（元数据未知时）。
pub fn register_id(source: &str, gd_id: &str) -> i32 {
    register_full(source, gd_id, "", "", "", 0.0)
}

/// 由 UI id 查回 (source, gd_id)。
pub fn lookup_id(ui_id: i32) -> Option<(String, String)> {
    lookup_full(ui_id).map(|(s, g, _, _, _, _)| (s, g))
}

/// 由 UI id 查回 (source, gd_id, name, artist, album, duration_sec)。
pub fn lookup_full(ui_id: i32) -> Option<(String, String, String, String, String, f32)> {
    ID_MAP.with(|m| {
        m.borrow()
            .iter()
            .find(|(id, _, _, _, _, _, _)| *id == ui_id)
            .map(|(_, src, gid, name, artist, album, dur)| {
                (src.clone(), gid.clone(), name.clone(), artist.clone(), album.clone(), *dur)
            })
    })
}

// ---- 本地收藏（web 内存态）：播放器 like-toggle 与"我的最爱"页共用同一集合 ----
// 注意：HashSet::new 非 const fn，thread_local 不能用 const{} 块初始化
thread_local! {
    static LIKED: RefCell<std::collections::HashSet<i32>> = RefCell::new(std::collections::HashSet::new());
}

pub fn liked_contains(ui_id: i32) -> bool {
    LIKED.with(|s| s.borrow().contains(&ui_id))
}
pub fn liked_insert(ui_id: i32) {
    LIKED.with(|s| {
        s.borrow_mut().insert(ui_id);
    });
    persist_write_liked();
}
pub fn liked_remove(ui_id: i32) {
    LIKED.with(|s| {
        s.borrow_mut().remove(&ui_id);
    });
    persist_write_liked();
}
pub fn liked_all() -> Vec<i32> {
    LIKED.with(|s| s.borrow().iter().copied().collect())
}

// ---- 最近播放（web 内存态，上限 100）：play.rs 播放时 push，"最近播放"页展示 ----
thread_local! {
    static RECENT: RefCell<Vec<(i32, String, String, String)>> = RefCell::new(Vec::new()); // (ui_id, name, artist, album)
}

pub fn recent_push(ui_id: i32, name: &str, artist: &str, album: &str) {
    RECENT.with(|r| {
        let mut g = r.borrow_mut();
        g.retain(|(id, _, _, _)| *id != ui_id);
        g.insert(0, (ui_id, name.to_string(), artist.to_string(), album.to_string()));
        if g.len() > 100 {
            g.truncate(100);
        }
    });
    persist_write_recent();
}
pub fn recent_list() -> Vec<(i32, String, String, String)> {
    RECENT.with(|r| r.borrow().clone())
}
pub fn recent_remove_ui(ui_id: i32) {
    RECENT.with(|r| {
        r.borrow_mut().retain(|(id, _, _, _)| *id != ui_id);
    });
    persist_write_recent();
}
pub fn recent_clear() {
    RECENT.with(|r| {
        r.borrow_mut().clear();
    });
    persist_write_recent();
}

// ---- 收藏 / 最近播放持久化（localStorage，键前缀 zplayer/）----
// 刷新后 UI id 全部重分配：持久化以 gd_id 为准（含元数据），`load_persisted()` 启动时
// register_full 重新登记 → 重建内存 LIKED/RECENT。每次变更即写（localStorage 同步 API）。

const LIKED_KEY: &str = "liked";
const RECENT_KEY: &str = "recent";

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistSong {
    source: String,
    gd_id: String,
    name: String,
    artist: String,
    album: String,
    duration: f32,
}

fn persist_song_from_ui(ui_id: i32) -> Option<PersistSong> {
    let (source, gd_id, name, artist, album, dur) = lookup_full(ui_id)?;
    Some(PersistSong {
        source,
        gd_id,
        name,
        artist,
        album,
        duration: dur,
    })
}

fn persist_write_liked() {
    let list: Vec<PersistSong> = LIKED
        .with(|s| s.borrow().iter().filter_map(|&id| persist_song_from_ui(id)).collect());
    let _ = crate::platform::storage::write_text(
        LIKED_KEY,
        &serde_json::to_string(&list).unwrap_or_default(),
    );
}

fn persist_write_recent() {
    let list: Vec<PersistSong> = RECENT
        .with(|r| r.borrow().iter().filter_map(|(id, _, _, _)| persist_song_from_ui(*id)).collect());
    let _ = crate::platform::storage::write_text(
        RECENT_KEY,
        &serde_json::to_string(&list).unwrap_or_default(),
    );
}

/// 启动恢复（`web::setup_all` 顶部调用，先于各页重建）：读 localStorage 重登记
/// 收藏/最近播放（分配新 UI id）。队列恢复见 play.rs（同键规则，gd_id 为准）。
pub fn load_persisted() {
    if let Some(txt) = crate::platform::storage::read_text(LIKED_KEY) {
        if let Ok(list) = serde_json::from_str::<Vec<PersistSong>>(&txt) {
            for s in list {
                let id = register_full(&s.source, &s.gd_id, &s.name, &s.artist, &s.album, s.duration);
                LIKED.with(|set| set.borrow_mut().insert(id));
            }
        }
    }
    if let Some(txt) = crate::platform::storage::read_text(RECENT_KEY) {
        if let Ok(list) = serde_json::from_str::<Vec<PersistSong>>(&txt) {
            for s in list {
                let id = register_full(&s.source, &s.gd_id, &s.name, &s.artist, &s.album, s.duration);
                recent_push(id, &s.name, &s.artist, &s.album);
            }
        }
    }
}

// ---- HTTP（web-sys fetch；gdmusic 回显任意 Origin，无 CORS 问题）----
// gdmusic 对并发限流敏感（实测并发封面拉取会放大 503）：全局并发闸 ≤4，
// 超出时 sleep 轮询等待（无 async 锁，用计数 + 退避实现）。
thread_local! {
    static FETCH_IN_FLIGHT: RefCell<u32> = const { RefCell::new(0) };
}

async fn acquire_fetch() {
    loop {
        let ok = FETCH_IN_FLIGHT.with(|c| {
            let mut g = c.borrow_mut();
            if *g < 4 {
                *g += 1;
                true
            } else {
                false
            }
        });
        if ok {
            break;
        }
        sleep_ms(120).await;
    }
}

fn release_fetch() {
    FETCH_IN_FLIGHT.with(|c| {
        let mut g = c.borrow_mut();
        *g = g.saturating_sub(1);
    });
}

/// gdmusic 不稳定（实测出现过 503/限流）：5xx/429 视为瞬时失败，内部最多重试 2 次
/// （600ms / 1200ms 退避），全部失败才返回 None，由调用方走空态/占位兜底。
pub async fn fetch_text(url: &str) -> Option<String> {
    fetch_text_inner(url, 0).await
}

// wasm32 async 递归需 Box::pin（E0733），用循环实现重试（无递归）
async fn fetch_text_inner(url: &str, mut attempt: u32) -> Option<String> {
    loop {
        acquire_fetch().await;
        let outcome = do_fetch_text(url).await;
        release_fetch();
        match outcome {
            Some((status, Some(text))) if status == 200 => return Some(text),
            // 5xx/429 与 CORS/网络失败（None）都是偶发瞬时：最多重试 2 次退避
            Some((status, _)) if status >= 500 || status == 429 => {
                if attempt >= 2 {
                    return None;
                }
                attempt += 1;
                sleep_ms(600 * attempt).await;
                continue;
            }
            Some(_) => return None,
            None => {
                // CORS/网络失败（fetch 被拒绝、超时等）：偶发，重试退避后仍失败才放弃
                if attempt >= 2 {
                    return None;
                }
                attempt += 1;
                sleep_ms(600 * attempt).await;
                continue;
            }
        }
    }
}

async fn do_fetch_text(url: &str) -> Option<(u16, Option<String>)> {
    let window = web_sys::window()?;
    let promise = window.fetch_with_str(url);
    let resp: web_sys::Response = JsFuture::from(promise).await.ok()?.dyn_into().ok()?;
    let status = resp.status();
    if status != 200 {
        return Some((status, None));
    }
    let text = JsFuture::from(resp.text().ok()?).await.ok()?;
    Some((status, text.as_string()))
}

/// wasm 环境 sleep（Promise + setTimeout 包装 JsFuture；无 tokio/timer 依赖）。
pub async fn sleep_ms(ms: u32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        let win = web_sys::window().unwrap();
        let f: js_sys::Function = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        })
        .unchecked_into();
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(&f, ms as i32);
    });
    let _ = JsFuture::from(p).await;
}

async fn get_json(url: &str) -> Option<serde_json::Value> {
    let txt = fetch_text(url).await?;
    serde_json::from_str(&txt).ok()
}

/// gdmusic 搜索（netease 主通道）：顶层 JSON 数组。
/// 空结果可能来自瞬时失败（gdmusic 会以 200+空体/错误体限流），重试 1 次。
pub async fn search(q: &str, count: u32, pages: u32) -> Vec<GdSong> {
    // 空查询不发请求（gdmusic 返回 400: Field `name` is required）
    if q.trim().is_empty() {
        return Vec::new();
    }
    for attempt in 0..2 {
        let url = format!(
            "https://music-api.gdstudio.xyz/api.php?types=search&source=netease&name={}&count={}&pages={}",
            js_sys::encode_uri_component(q.trim()),
            count,
            pages
        );
        let txt = fetch_text(&url).await.unwrap_or_default();
        let arr: Vec<serde_json::Value> = serde_json::from_str(&txt).unwrap_or_default();
        if !arr.is_empty() || attempt == 1 {
            return arr.into_iter().map(gd_song_from_json).collect();
        }
        sleep_ms(700).await;
    }
    Vec::new()
}

/// 多源搜索兜底：netease → joox → tidal，取第一个非空结果。
pub async fn search_multi(q: &str, count: u32, pages: u32) -> Vec<GdSong> {
    if q.trim().is_empty() {
        return Vec::new();
    }
    for src in ["netease", "joox", "tidal"] {
        let url = format!(
            "https://music-api.gdstudio.xyz/api.php?types=search&source={}&name={}&count={}&pages={}",
            src,
            js_sys::encode_uri_component(q.trim()),
            count,
            pages
        );
        if let Some(txt) = fetch_text(&url).await {
            let arr: Vec<serde_json::Value> = serde_json::from_str(&txt).unwrap_or_default();
            if !arr.is_empty() {
                return arr.into_iter().map(gd_song_from_json).collect();
            }
        }
    }
    Vec::new()
}

fn gd_song_from_json(v: serde_json::Value) -> GdSong {
    let artist = v
        .get("artist")
        .and_then(|a| a.as_array())
        .and_then(|a| a.first())
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    GdSong {
        id: str_field(&v, "id"),
        name: str_field(&v, "name"),
        artist,
        album: str_field(&v, "album"),
        pic_id: str_field(&v, "pic_id"),
        url_id: str_field(&v, "url_id"),
        lyric_id: str_field(&v, "lyric_id"),
        source: str_field(&v, "source"),
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .map(|x| if let Some(s) = x.as_str() { s.to_string() } else { x.to_string() })
        .unwrap_or_default()
}

/// 可播 URL（br 默认 320 = mp3；999 返回 flac 浏览器兼容差）。
/// url 端点同样不稳定（实测 503），空 url 视为瞬时失败重试 1 次。
pub async fn song_url(id: &str, source: &str, br: &str) -> Option<String> {
    for attempt in 0..2 {
        let url = format!(
            "https://music-api.gdstudio.xyz/api.php?types=url&source={}&id={}&br={}",
            js_sys::encode_uri_component(source),
            js_sys::encode_uri_component(id),
            br
        );
        if let Some(v) = get_json(&url).await {
            let u = v
                .get("url")
                .and_then(|u| u.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            if u.is_some() || attempt == 1 {
                return u;
            }
        } else if attempt == 1 {
            return None;
        }
        sleep_ms(700).await;
    }
    None
}

/// 歌单详情（{"playlist":{name,coverImgUrl,description,tracks[]}}）；tracks 取前 limit 首
/// （详情响应可达 470KB+，截断控制解析成本与 UI 模型体积）。
/// 结构性空结果（无 playlist 键 / tracks 为空）视为瞬时失败重试 1 次 —— 固定榜单
/// 歌单（60 首起）不可能真实为空，空体 = gdmusic 限流/503 的另一种表现。
pub async fn playlist_detail(id: &str, limit: usize) -> Option<GdPlaylist> {
    for attempt in 0..2 {
        let url = format!(
            "https://music-api.gdstudio.xyz/api.php?types=playlist&source=netease&id={}",
            js_sys::encode_uri_component(id)
        );
        if let Some(v) = get_json(&url).await {
            if let Some(pl) = v.get("playlist") {
                if let Some(tracks) = pl.get("tracks").and_then(|t| t.as_array()) {
                    if !tracks.is_empty() || attempt == 1 {
                        let mut g = build_playlist(id, pl, limit);
                        return Some(g);
                    }
                }
            }
        }
        if attempt == 1 {
            return None;
        }
        sleep_ms(700).await;
    }
    None
}

fn build_playlist(
    id: &str,
    pl: &serde_json::Value,
    limit: usize,
) -> GdPlaylist {
    let mut g = GdPlaylist {
        id: id.to_string(),
        name: pl.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        cover_url: pl.get("coverImgUrl").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        description: pl.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        tracks: Vec::new(),
    };
    if let Some(ts) = pl.get("tracks").and_then(|t| t.as_array()) {
        for t in ts.iter().take(limit) {
            let dur = t.get("dt").and_then(|d| d.as_f64()).unwrap_or(0.0) / 1000.0;
            g.tracks.push(GdTrack {
                id: str_field(t, "id"),
                name: str_field(t, "name"),
                artist: t
                    .get("ar")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|a| a.get("name"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                album: t
                    .get("al")
                    .and_then(|a| a.get("name"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                cover_url: https_cover(
                    t.get("al")
                        .and_then(|a| a.get("picUrl"))
                        .and_then(|x| x.as_str())
                        .unwrap_or(""),
                ),
                duration_sec: dur as f32,
            });
        }
    }
    g
}

/// LRC 歌词（{"lyric":"<lrc>"}）。
pub async fn lyric(id: &str) -> Option<String> {
    let url = format!(
        "https://music-api.gdstudio.xyz/api.php?types=lyric&source=netease&id={}",
        js_sys::encode_uri_component(id)
    );
    let v = get_json(&url).await?;
    v.get("lyric")
        .and_then(|l| l.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// pic_id → 完整封面 URL（{"url":"https://p2.music.126.net/..."}）。
/// 注意：浏览器直连 126 图片 CDN 被 CORS/反盗链阻断（探测 503），阶段 1b 封面展示走
/// 渐变兜底；本函数供后续代理解码方案接入。
pub async fn pic_url(pic_id: &str) -> Option<String> {
    let url = format!(
        "https://music-api.gdstudio.xyz/api.php?types=pic&source=netease&id={}",
        js_sys::encode_uri_component(pic_id)
    );
    let v = get_json(&url).await?;
    v.get("url")
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// http → https（浏览器 https 页面会拦截混合内容）。
pub fn https_cover(url: &str) -> String {
    if url.starts_with("http://") {
        format!("https{}", &url[4..])
    } else {
        url.to_string()
    }
}

/// 封面 URL → UI 展示 image。同步兜底：缓存命中返回真实图，未加载/失败返回空图
/// （组件渐变兜底）。异步加载请用 `spawn_cover_url / spawn_cover_pic`。
pub fn cover_image(_cover_url: &str) -> slint::Image {
    if let Some(img) = COVER_CACHE.with(|c| c.borrow().get(_cover_url).cloned()) {
        return img;
    }
    slint::Image::default()
}

// ---- 封面加载：fetch 字节 → image crate 解码 → slint::Image（内存缓存去重）----
thread_local! {
    /// 封面 URL → 解码后 Image（去重：同一封面多处引用只拉一次）。
    static COVER_CACHE: RefCell<std::collections::HashMap<String, slint::Image>> = RefCell::new(std::collections::HashMap::new());
    /// pic_id → 封面 URL（去重 types=pic 调用）。
    static PIC_URL_CACHE: RefCell<std::collections::HashMap<String, String>> = RefCell::new(std::collections::HashMap::new());
}

/// 图片字节 → slint::Image（image crate 解码 + 最长边 ≤512 等比缩放 + RGBA8）。
pub fn decode_image(bytes: &[u8]) -> Option<slint::Image> {
    use image::GenericImageView;
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = img.dimensions();
    let long = w.max(h);
    let img = if long > 512 {
        let k = 512.0 / long as f32;
        let nw = (w as f32 * k).max(1.0) as u32;
        let nh = (h as f32 * k).max(1.0) as u32;
        img.resize(nw, nh, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (w2, h2) = img.dimensions();
    let rgba = img.to_rgba8();
    let buf =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), w2, h2);
    Some(slint::Image::from_rgba8(buf))
}

/// 二进制 GET（封面字节；gdmusic/126 图床均可，CORS 已实测开放）。受全局并发闸约束。
pub async fn fetch_bytes(url: &str) -> Option<Vec<u8>> {
    // 封面字节偶发 CORS/5xx：失败重试（≤3 次退避），全部失败返回 None 由调用方渐变兜底
    for attempt in 0..3u32 {
        acquire_fetch().await;
        let result = do_fetch_bytes(url).await;
        release_fetch();
        if result.is_some() {
            return result;
        }
        if attempt >= 2 {
            break;
        }
        sleep_ms(600 * (attempt + 1)).await;
    }
    None
}

async fn do_fetch_bytes(url: &str) -> Option<Vec<u8>> {
    let window = web_sys::window()?;
    let promise = window.fetch_with_str(url);
    let resp: web_sys::Response = JsFuture::from(promise).await.ok()?.dyn_into().ok()?;
    if resp.status() != 200 {
        return None;
    }
    let buf = JsFuture::from(resp.array_buffer().ok()?).await.ok()?;
    Some(js_sys::Uint8Array::new(&buf).to_vec())
}

/// 封面 URL → Image（缓存命中直接返回；失败 None → 调用方保持渐变兜底）。
pub async fn fetch_cover_url(url: &str) -> Option<slint::Image> {
    if url.is_empty() {
        return None;
    }
    if let Some(img) = COVER_CACHE.with(|c| c.borrow().get(url).cloned()) {
        return Some(img);
    }
    let bytes = fetch_bytes(url).await?;
    let img = decode_image(&bytes)?;
    COVER_CACHE.with(|c| c.borrow_mut().insert(url.to_string(), img.clone()));
    Some(img)
}

/// pic_id → 封面 Image（内部经 types=pic 取 URL；URL 缓存去重）。
pub async fn fetch_cover_pic(pic_id: &str) -> Option<slint::Image> {
    if pic_id.is_empty() {
        return None;
    }
    let url = if let Some(u) = PIC_URL_CACHE.with(|c| c.borrow().get(pic_id).cloned()) {
        u
    } else {
        // types=pic 偶发 CORS/5xx：失败重试一次再放弃（稳定阻断则由调用方保持渐变兜底）
        let mut u = pic_url(pic_id).await;
        if u.is_none() {
            u = pic_url(pic_id).await;
        }
        let u = u?;
        PIC_URL_CACHE.with(|c| c.borrow_mut().insert(pic_id.to_string(), u.clone()));
        u
    };
    fetch_cover_url(&url).await
}

/// 业务接线入口：异步加载 URL 封面，完成后回调（模块内更新模型行 cover）。
pub fn spawn_cover_url(url: String, on_done: impl FnOnce(Option<slint::Image>) + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        on_done(fetch_cover_url(&url).await);
    });
}

/// 业务接线入口：异步按 pic_id 加载封面（内部取 URL + 解码 + 缓存）。
pub fn spawn_cover_pic(pic_id: String, on_done: impl FnOnce(Option<slint::Image>) + 'static) {
    wasm_bindgen_futures::spawn_local(async move {
        on_done(fetch_cover_pic(&pic_id).await);
    });
}
