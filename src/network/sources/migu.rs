//! 咪咕音乐音源（无复杂签名，接口较稳定）

use super::{MultiSourceClient, ResolvedSong, SourceSong};

pub const NAME: &str = "migu";

struct SongInfo {
    cid: String,
    album_id: String,
    name: String,
    artist: String,
    album: String,
    cover: String,
    duration: Option<u64>,
}

/// 解析时长值：数字秒 或 数字字符串
fn parse_duration(v: &serde_json::Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|x| x.trim().parse().ok()))
}

/// 聚合搜索：返回该源歌曲条目列表
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let songs = search(c, kw).await?;
    Some(
        songs
            .into_iter()
            .map(|s| SourceSong {
                source: NAME,
                id: s.cid,
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: s.cover,
                duration: s.duration,
            })
            .collect(),
    )
}

/// 按 id 精确解析播放 URL（聚合条目的播放入口）
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    name: &str,
    artist: &str,
) -> Option<String> {
    let kw = if artist.is_empty() { name.to_string() } else { format!("{} {}", name, artist) };
    let songs = search(c, &kw).await?;
    let pick = songs.into_iter().find(|s| s.cid == id)?;
    get_play_url(c, &pick.cid, &pick.album_id).await
}

pub async fn try_resolve(c: &MultiSourceClient, title: &str, artist: &str) -> Option<ResolvedSong> {
    let kw = if artist.is_empty() { title.to_string() } else { format!("{} {}", title, artist) };
    let songs = search(c, &kw).await?;

    let pick = songs.into_iter().find(|s| {
        let name_ok = s.name.contains(title) || title.contains(&s.name);
        let artist_ok = artist.is_empty()
            || s.artist.is_empty()
            || s.artist.contains(artist)
            || artist.contains(&s.artist);
        name_ok && artist_ok
    })?;

    let url = get_play_url(c, &pick.cid, &pick.album_id).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(pick.cid),
        duration: pick.duration,
    })
}

async fn search(c: &MultiSourceClient, kw: &str) -> Option<Vec<SongInfo>> {
    let url = format!(
        "https://m.music.migu.cn/migu/remoting/scr_search_tag?rows=20&type=2&keyword={}&pgc=1",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Referer", "https://m.music.migu.cn/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let songs = extract_songs(&json);
    if songs.is_empty() {
        return None;
    }

    let out: Vec<SongInfo> = songs
        .into_iter()
        .filter_map(|s| {
            let cid = s.get("cid").and_then(|v| v.as_str()).or_else(|| s.get("contentId").and_then(|v| v.as_str()))?;
            let name = s.get("songName").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if name.is_empty() {
                return None;
            }
            let artist = s.get("singer")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|x| x.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let album_id = s.get("albumId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let album = s.get("albumName")
                .and_then(|v| v.as_str())
                .filter(|a| !a.is_empty())
                .or_else(|| s.get("album").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let cover = s.get("albumPic")
                .and_then(|v| v.as_str())
                .filter(|p| !p.is_empty())
                .or_else(|| s.get("pic").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            let duration = s.get("duration")
                .and_then(parse_duration)
                .or_else(|| s.get("length").and_then(parse_duration));
            Some(SongInfo { cid: cid.to_string(), album_id, name, artist, album, cover, duration })
        })
        .collect();
    if out.is_empty() { None } else { Some(out) }
}

fn extract_songs(json: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut arrs: Vec<&serde_json::Value> = Vec::new();
    if let Some(result) = json.get("result") {
        if let Some(rr) = result.get("result") {
            if let Some(arr) = rr.as_array() {
                for item in arr {
                    if let Some(songs) = item.get("songs") {
                        arrs.push(songs);
                    }
                }
            }
            if let Some(songs) = result.get("songs") {
                arrs.push(songs);
            }
        }
        if let Some(arr) = result.as_array() {
            for item in arr {
                if let Some(songs) = item.get("songs") {
                    arrs.push(songs);
                }
            }
        }
        if let Some(songs) = result.get("songs") {
            arrs.push(songs);
        }
    }
    for arr in arrs {
        if let Some(a) = arr.as_array() {
            let items: Vec<serde_json::Value> = a.iter().cloned().collect();
            if !items.is_empty() {
                return items;
            }
        }
    }
    Vec::new()
}

async fn get_play_url(c: &MultiSourceClient, cid: &str, album_id: &str) -> Option<String> {
    let form = [
        ("toneFlag", "1"),
        ("netType", "01"),
        ("userId", "1554861458871010208504"),
        ("songId", cid),
        ("albumId", album_id),
    ];
    let resp = c.http.post("https://app.pd.nf.migu.cn/MIGUM3.0/v1.0/content/sub/listenSong.do")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .form(&form)
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    let data = json.get("data")?;
    if let Some(urls) = data.get("urls").and_then(|v| v.as_array()) {
        for u in urls {
            if let Some(u) = u.get("url").and_then(|v| v.as_str()) {
                if !u.is_empty() && u.starts_with("http") {
                    return Some(u.to_string());
                }
            }
        }
    }
    for key in ["url", "playUrl"] {
        if let Some(u) = data.get(key).and_then(|v| v.as_str()) {
            if !u.is_empty() && u.starts_with("http") {
                return Some(u.to_string());
            }
        }
    }
    None
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
