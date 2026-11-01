//! YouTube 音源（国外兜底）：取 `youtubei/v1/player` 的 adaptiveFormats 纯音频流。
//! 直接拿音频自适应流 URL，不经过官方嵌入播放器/HLS，因此不会插入广告段。
//! 注意：部分流地址带 `n` 签名参数，若服务端强制验证会 403；此处未做签名解密，
//! 失败时上层会继续尝试后续音源（SoundCloud 等）。

use super::{MultiSourceClient, ResolvedSong, SourceSong};
use serde_json::{json, Value};

pub const NAME: &str = "youtube";

// YouTube 网页端公开的 innertube API key
const INNERTUBE_KEY: &str = "AIzaSyAO_FJ2SlqU8Q4STEHLGCilw_Y9_11qcW8";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

fn context() -> Value {
    json!({
        "client": { "clientName": "WEB", "clientVersion": "2.20240101.00.00", "hl": "en", "gl": "US" }
    })
}

fn music_context() -> Value {
    json!({
        "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20241001.01.00", "hl": "en", "gl": "US" }
    })
}

/// YouTube Music 搜索请求
async fn music_search_json(c: &MultiSourceClient, kw: &str) -> Option<Value> {
    let url = format!(
        "https://music.youtube.com/youtubei/v1/search?alt=json&key={}&prettyPrint=false",
        INNERTUBE_KEY
    );
    let body = json!({ "context": music_context(), "query": kw });
    let resp = c.http
        .post(&url)
        .header("User-Agent", UA)
        .json(&body)
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

pub async fn try_resolve(c: &MultiSourceClient, title: &str, artist: &str) -> Option<ResolvedSong> {
    let kw = if artist.is_empty() { title.to_string() } else { format!("{} {}", title, artist) };
    let video_id = search(c, &kw).await?;
    let url = get_audio(c, &video_id).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(video_id),
        duration: None,
    })
}

async fn search(c: &MultiSourceClient, kw: &str) -> Option<String> {
    let json = search_json(c, kw).await?;
    find_video_id(&json)
}

/// 聚合搜索：走 YouTube Music（music.youtube.com）返回音乐条目，而非全站视频
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let json = music_search_json(c, kw).await?;
    let mut out: Vec<SourceSong> = Vec::new();
    collect_music(&json, &mut out);
    if out.is_empty() { None } else { Some(out) }
}

/// 按 id（videoId）精确解析播放 URL
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    _name: &str,
    _artist: &str,
) -> Option<String> {
    get_audio(c, id).await
}

async fn search_json(c: &MultiSourceClient, kw: &str) -> Option<Value> {
    let url = format!(
        "https://www.youtube.com/youtubei/v1/search?key={}&prettyPrint=false",
        INNERTUBE_KEY
    );
    let body = json!({ "context": context(), "query": kw });
    let resp = c.http
        .post(&url)
        .header("User-Agent", UA)
        .json(&body)
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json().await.ok()
}

/// 深度遍历，找到第一个 videoRenderer.videoId（搜索结果）。
/// 递归优先在对象值中找，命中概率高的常规结果即返回。
fn find_video_id(v: &Value) -> Option<String> {
    if let Some(vr) = v.get("videoRenderer") {
        if let Some(id) = vr.get("videoId").and_then(|x| x.as_str()) {
            return Some(id.to_string());
        }
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(id) = find_video_id(item) {
                return Some(id);
            }
        }
        return None;
    }
    if let Some(obj) = v.as_object() {
        for val in obj.values() {
            if let Some(id) = find_video_id(val) {
                return Some(id);
            }
        }
    }
    None
}

async fn get_audio(c: &MultiSourceClient, video_id: &str) -> Option<String> {
    let url = format!(
        "https://www.youtube.com/youtubei/v1/player?key={}&prettyPrint=false",
        INNERTUBE_KEY
    );
    let body = json!({ "context": context(), "videoId": video_id });
    let resp = c.http
        .post(&url)
        .header("User-Agent", UA)
        .json(&body)
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    let fmts = json
        .get("streamingData")
        .and_then(|s| s.get("adaptiveFormats"))
        .and_then(|a| a.as_array())?;
    // 优先 audio/mp4（itag 140，m4a，兼容性最好），其次 audio/webm（itag 251，opus）
    let mut fallback: Option<String> = None;
    for f in fmts {
        let mime = f.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
        let u = f.get("url").and_then(|x| x.as_str()).unwrap_or("");
        if u.is_empty() {
            continue;
        }
        if mime.contains("audio/mp4") {
            return Some(u.to_string());
        }
        if mime.contains("audio/webm") && fallback.is_none() {
            fallback = Some(u.to_string());
        }
    }
    fallback
}

/// 深度收集所有 videoRenderer 为歌曲条目
#[allow(dead_code)] // 备用：当前 search 用主解析路径，深度收集保留供未来扩展
fn collect_videos(v: &Value, out: &mut Vec<SourceSong>) {
    if let Some(vr) = v.get("videoRenderer") {
        if let Some(id) = vr.get("videoId").and_then(|x| x.as_str()) {
            let title = vr
                .pointer("/title/runs/0/text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let artist = vr
                .pointer("/ownerText/runs/0/text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let mut cover = "";
            if let Some(th) = vr
                .get("thumbnail")
                .and_then(|t| t.get("thumbnails"))
                .and_then(|t| t.as_array())
            {
                if let Some(last) = th.last() {
                    cover = last.get("url").and_then(|x| x.as_str()).unwrap_or("");
                }
            }
            let duration = vr
                .get("lengthText")
                .and_then(|l| l.get("simpleText"))
                .and_then(|x| x.as_str())
                .and_then(duration_secs);
            out.push(SourceSong {
                source: NAME,
                id: id.to_string(),
                name: title,
                artist,
                album: String::new(),
                cover: cover.to_string(),
                duration,
            });
            return;
        }
    }
    if let Some(arr) = v.as_array() {
        for it in arr {
            collect_videos(it, out);
        }
        return;
    }
    if let Some(obj) = v.as_object() {
        for val in obj.values() {
            collect_videos(val, out);
        }
    }
}

/// 深度收集所有 musicResponsiveListItemRenderer（YT Music 歌曲条目）
fn collect_music(v: &Value, out: &mut Vec<SourceSong>) {
    if let Some(itr) = v.get("musicResponsiveListItemRenderer") {
        if let Some(s) = parse_music_item(itr) {
            out.push(s);
        }
        return;
    }
    if let Some(arr) = v.as_array() {
        for it in arr {
            collect_music(it, out);
        }
        return;
    }
    if let Some(obj) = v.as_object() {
        for val in obj.values() {
            collect_music(val, out);
        }
    }
}

/// 解析一条 YT Music 歌曲条目
fn parse_music_item(itr: &Value) -> Option<SourceSong> {
    let video_id = itr
        .pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;
    let name = itr
        .pointer("/flexColumns/0/musicResponsiveListItemFlexColumnRenderer/text/runs/0/text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return None;
    }
    // 第二列 runs 拼接为「歌手 · 专辑」，用 · 拆开
    let mut artist = String::new();
    let mut album = String::new();
    if let Some(runs) = itr
        .pointer("/flexColumns/1/musicResponsiveListItemFlexColumnRenderer/text/runs")
        .and_then(|v| v.as_array())
    {
        let joined: String = runs
            .iter()
            .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("");
        let parts: Vec<&str> = joined.split('·').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if let Some(a) = parts.first() {
            artist = a.to_string();
        }
        if parts.len() > 1 {
            album = parts[1].to_string();
        }
    }
    let mut cover = String::new();
    if let Some(th) = itr
        .get("thumbnail")
        .and_then(|t| t.get("musicThumbnailRenderer"))
        .and_then(|t| t.get("thumbnail"))
        .and_then(|t| t.get("thumbnails"))
        .and_then(|t| t.as_array())
    {
        if let Some(last) = th.last() {
            cover = last.get("url").and_then(|x| x.as_str()).unwrap_or("").to_string();
        }
    }
    let duration = itr
        .get("fixedColumns")
        .and_then(|fc| fc.as_array())
        .and_then(|a| a.get(0))
        .and_then(|c| c.get("musicResponsiveListItemFixedColumnRenderer"))
        .and_then(|c| c.get("text"))
        .and_then(|t| t.get("runs"))
        .and_then(|r| r.as_array())
        .and_then(|r| r.get(0))
        .and_then(|x| x.get("text"))
        .and_then(|x| x.as_str())
        .and_then(duration_secs);
    Some(SourceSong {
        source: NAME,
        id: video_id,
        name,
        artist,
        album,
        cover,
        duration,
    })
}

/// "3:45" / "1:02:30" -> 秒
fn duration_secs(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split(':').collect();
    let mut total = 0u64;
    for p in &parts {
        let n: u64 = p.trim().parse().ok()?;
        total = total * 60 + n;
    }
    Some(total)
}
