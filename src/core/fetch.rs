//! 音频字节获取（播放/预取共用）+ 网络响应解析工具。
//! 正式播放与后台预取共用 fetch_song_bytes，使点上一首/下一首时目标歌曲
//! 大概率已在缓存中，从而近乎立即出声。

use crate::network::netease::client::NeteaseClient;

/// 歌单分页加载状态（滚动触底追加下一页）。
/// items 只存 Send 字段（封面在 UI 线程由这些字段构造），保证跨线程安全。
pub(crate) struct Paging {
    pub cat: i32,
    pub offset: u32,
    /// 分类切换代数：切分类时 +1，旧分类的迟到响应据此丢弃（避免串页/错位）
    pub epoch: u64,
    pub end: bool,
    pub loading: bool,
    pub items: Vec<(i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color)>,
}

/// 下载专用 Client：全局复用（切歌/预取不再各自重建连接池与 TLS 握手），
/// 按代理配置版本号自动重建，使设置页改代理后即时生效；带读超时防读挂死。
///
/// 注意：wasm32 走 web-sys fetch，不存在 reqwest，本函数只在桌面/移动分支编译。
#[cfg(not(target_arch = "wasm32"))]
fn download_client() -> reqwest::Client {
    use std::sync::{Mutex, OnceLock};
    static C: OnceLock<Mutex<(u64, reqwest::Client)>> = OnceLock::new();
    let v = crate::network::proxy::current_version();
    let cell = C.get_or_init(|| Mutex::new((v, crate::network::proxy::build_download_client())));
    let mut g = cell.lock().unwrap();
    if g.0 != v {
        g.1 = crate::network::proxy::build_download_client();
        g.0 = v;
    }
    g.1.clone()
}

/// 获取歌曲音频字节：磁盘缓存命中直接返回；否则解析可播 URL（主源 → 多音源
/// 兜底）下载并写入缓存。
pub(crate) async fn fetch_song_bytes(
    client: &NeteaseClient,
    uid: i64,
    name: &str,
    artist: &str,
    dur_sec: u64,
) -> Option<Vec<u8>> {
    if let Some(b) = crate::persist::cache::load_song(uid) {
        return Some(b);
    }
    // 主源优先：网易云直链实际下载成功即用（避免每首都白跑 7 源兜底探测）
    let http = download_client();
    if let Ok(v) = crate::network::service::song_url(client, &[uid], 999000).await {
        if let Some(arr) = v.as_array() {
            for d in arr {
                if let Some(u) = d.get("url").and_then(|x| x.as_str()) {
                    if u.is_empty() { continue; }
                    if let Some(b) = try_download(&http, u).await {
                        // 写盘 + 淘汰扫描进阻塞池，不在 async worker 上做整首音频 IO
                        return crate::persist::cache::save_song_bg(uid, b).await;
                    }
                    break; // 主源首条 url 下载失败即换兜底
                }
            }
        }
    }
    // 主源失败 → 兜底源（resolve 已逐源内容级探测可播，且放宽时长差）
    if let Ok(v) = crate::network::service::resolve_fallback(name, artist, Some(dur_sec)).await {
        if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
            if !u.is_empty() {
                if let Some(b) = try_download(&http, u).await {
                    return crate::persist::cache::save_song_bg(uid, b).await;
                }
            }
        }
    }
    // 洛雪音源脚本兜底（用户导入 .js，网易云直链 + 内置 7 源都失败后逐源解析）。
    // 仅非 wasm（desktop/mobile）：web 下 rquickjs 无法编译，不启用。
    #[cfg(not(target_arch = "wasm32"))]
    {
        if crate::network::sources::lx_scripts::enabled_any() {
            let info = crate::network::sources::lx_scripts::music_info(uid, name, artist, "", dur_sec);
            let quality = crate::persist::config::AppConfig::load().play.quality_index;
            if let Some(u) = crate::network::sources::lx_scripts::resolve(&info, quality).await {
                if let Some(b) = try_download(&http, &u).await {
                    return crate::persist::cache::save_song_bg(uid, b).await;
                }
            }
        }
    }
    None
}

/// 尝试从 URL 下载非空音频字节（下载失败/空内容视为不可播）。
async fn try_download(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let Ok(resp) = http.get(url).send().await else { return None; };
    let Ok(b) = resp.bytes().await else { return None; };
    if b.is_empty() { return None; }
    Some(b.to_vec())
}

/// 解析搜索建议：优先 allMatch（新格式），回退到 songs/artists/albums/playlists 分类结构。
pub(crate) fn parse_suggest_items(v: &serde_json::Value) -> Vec<(String, String)> {
    let kind_of = |t: i64| -> &'static str {
        match t {
            1 => "song",
            100 => "artist",
            10 => "album",
            1000 => "playlist",
            1004 => "mv",
            1006 => "lyric",
            1009 => "radio",
            _ => "keyword",
        }
    };
    if let Some(arr) = v.pointer("/result/allMatch").and_then(|a| a.as_array()) {
        let list: Vec<(String, String)> = arr
            .iter()
            .filter_map(|m| {
                let k = m.get("keyword").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if k.is_empty() {
                    return None;
                }
                let t = m.get("type").and_then(|x| x.as_i64()).unwrap_or(0);
                Some((k, kind_of(t).to_string()))
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    let mut out = Vec::new();
    for (key, kind) in [("songs", "song"), ("artists", "artist"), ("albums", "album"), ("playlists", "playlist")] {
        if let Some(arr) = v.pointer(&format!("/result/{}", key)).and_then(|a| a.as_array()) {
            for item in arr {
                let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !name.is_empty() {
                    out.push((name, kind.to_string()));
                    if out.len() >= 8 {
                        break;
                    }
                }
            }
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}
