//! SoundCloud 音源（国外兜底/聚合）：取 progressive mp3 音频流。
//! 用公开的 api-v2 搜索 + track 的 progressive transcoding（原生 audio 可直接播放，
//! 不采用 HLS m3u8，因为浏览器原生不支持）。

use super::{MultiSourceClient, ResolvedSong, SourceSong};
use serde_json::Value;

pub const NAME: &str = "soundcloud";

// SoundCloud 公开嵌入 SDK 使用的 client_id（官方会不定期轮换，失效时需更新此值）
const CLIENT_ID: &str = "k3S1sW0EdDabcmSElfdbtXzmZOXVWsMv";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

struct TrackInfo {
    id: u64,
    title: String,
    artist: String,
    stream_url: String,
    duration: Option<u64>,
}

/// 聚合搜索：返回该源歌曲条目列表
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let url = format!(
        "https://api-v2.soundcloud.com/search/tracks?client_id={}&q={}&limit=10&app_locale=en",
        CLIENT_ID,
        urlencoding(kw)
    );
    let resp = c.http
        .get(&url)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    let collection = json.get("collection").and_then(|v| v.as_array())?;
    let out: Vec<SourceSong> = collection
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|v| v.as_u64())?;
            let name = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                return None;
            }
            let artist = item
                .get("user")
                .and_then(|u| u.get("username"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cover = item.get("artwork_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let duration = item.get("duration").and_then(|v| v.as_u64()).map(|d| d / 1000);
            Some(SourceSong {
                source: NAME,
                id: id.to_string(),
                name,
                artist,
                album: String::new(),
                cover,
                duration,
            })
        })
        .collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 按 id 精确解析播放 URL（直接查 track 详情取 progressive 流）
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    _name: &str,
    _artist: &str,
) -> Option<String> {
    let url = format!(
        "https://api-v2.soundcloud.com/tracks/{}?client_id={}",
        id, CLIENT_ID
    );
    let resp = c.http.get(&url).header("User-Agent", UA).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    if let Some(trs) = json.get("media").and_then(|m| m.get("transcodings")).and_then(|t| t.as_array()) {
        for tr in trs {
            let proto = tr
                .get("format")
                .and_then(|f| f.get("protocol"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let u = tr.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if proto == "progressive" && !u.is_empty() {
                return stream_with_client(c, u).await;
            }
        }
    }
    if let Some(u) = json.get("download_url").and_then(|v| v.as_str()) {
        if !u.is_empty() {
            return stream_with_client(c, u).await;
        }
    }
    None
}

/// 给转码/下载地址拼 client_id，并跟随重定向拿到最终流地址
async fn stream_with_client(c: &MultiSourceClient, raw: &str) -> Option<String> {
    let url = if raw.contains('?') {
        format!("{}&client_id={}", raw, CLIENT_ID)
    } else {
        format!("{}?client_id={}", raw, CLIENT_ID)
    };
    let resp = c.http.get(&url).header("User-Agent", UA).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    Some(resp.url().to_string())
}

pub async fn try_resolve(c: &MultiSourceClient, title: &str, artist: &str) -> Option<ResolvedSong> {
    let kw = if artist.is_empty() { title.to_string() } else { format!("{} {}", title, artist) };
    let track = search(c, &kw, title, artist).await?;
    let url = get_stream(c, &track).await?;
    Some(ResolvedSong {
        url,
        source: NAME,
        name: track.title.clone(),
        artist: track.artist.clone(),
        platform_id: Some(track.id.to_string()),
        duration: track.duration,
    })
}

async fn search(
    c: &MultiSourceClient,
    kw: &str,
    title: &str,
    artist: &str,
) -> Option<TrackInfo> {
    let url = format!(
        "https://api-v2.soundcloud.com/search/tracks?client_id={}&q={}&limit=10&app_locale=en",
        CLIENT_ID,
        urlencoding(kw)
    );
    let resp = c.http
        .get(&url)
        .header("User-Agent", UA)
        .header("Accept", "application/json")
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: Value = resp.json().await.ok()?;
    let collection = json.get("collection").and_then(|v| v.as_array())?;

    for item in collection {
        let id = match item.get("id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => continue,
        };
        let name = item.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let singer = item
            .get("user")
            .and_then(|u| u.get("username"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let dur = item.get("duration").and_then(|v| v.as_u64()).map(|d| d / 1000);
        // 标题匹配（任一方向包含），歌手尽量匹配
        let name_ok = name.contains(title) || title.contains(&name);
        if !name_ok {
            continue;
        }
        let artist_ok = artist.is_empty() || singer.is_empty()
            || singer.contains(artist) || artist.contains(&singer);
        if !artist_ok {
            continue;
        }

        // 找 progressive（mp3）转码
        let transcodings = item
            .get("media")
            .and_then(|m| m.get("transcodings"))
            .and_then(|t| t.as_array());
        if let Some(trs) = transcodings {
            for tr in trs {
                let proto = tr
                    .get("format")
                    .and_then(|f| f.get("protocol"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let u = tr.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if proto == "progressive" && !u.is_empty() {
                    return Some(TrackInfo { id, title: name, artist: singer, stream_url: u.to_string(), duration: dur });
                }
            }
        }
        // 无 progressive，尝试 download_url（部分 track 提供）
        if let Some(u) = item.get("download_url").and_then(|v| v.as_str()) {
            if !u.is_empty() {
                return Some(TrackInfo { id, title: name, artist: singer, stream_url: u.to_string(), duration: dur });
            }
        }
    }
    None
}

async fn get_stream(c: &MultiSourceClient, track: &TrackInfo) -> Option<String> {
    // 给转码/下载地址拼上 client_id
    let url = if track.stream_url.contains('?') {
        format!("{}&client_id={}", track.stream_url, CLIENT_ID)
    } else {
        format!("{}?client_id={}", track.stream_url, CLIENT_ID)
    };
    let resp = c.http.get(&url)
        .header("User-Agent", UA)
        .send()
        .await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let final_url = resp.url().to_string();
    Some(final_url)
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "%20".to_string(),
            _ => format!("%{:02X}", b),
        })
        .collect()
}
