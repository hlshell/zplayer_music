//! Joox（腾讯港澳/东南亚音乐）音源；公开接口，无需 token 签名（对齐 unblock 的 joox.js）
//! search: api-jooxtt.sanook.com/web_search  /  track: api.joox.com/web_get_songinfo

use super::{MultiSourceClient, ResolvedSong, SourceSong};
use base64::{engine::general_purpose::STANDARD, Engine};

pub const NAME: &str = "joox";

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";

struct SongInfo {
    songid: String,
    name: String,
    artist: String,
    album: String,
    duration: Option<u64>,
}

/// joox 的 info1/info3/singer.name 均为 base64
fn b64(s: &str) -> String {
    STANDARD
        .decode(s)
        .ok()
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default()
}

/// 聚合搜索：返回该源歌曲条目列表
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let songs = search(c, kw).await?;
    Some(
        songs
            .into_iter()
            .map(|s| SourceSong {
                source: NAME,
                id: s.songid,
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: String::new(),
                duration: s.duration,
            })
            .collect(),
    )
}

/// 按 id 精确解析播放 URL
pub async fn resolve_url_by_id(c: &MultiSourceClient, id: &str, _name: &str, _artist: &str) -> Option<String> {
    track(c, id).await
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
    let url = track(c, &pick.songid).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(pick.songid),
        duration: pick.duration,
    })
}

async fn search(c: &MultiSourceClient, kw: &str) -> Option<Vec<SongInfo>> {
    let url = format!(
        "http://api-jooxtt.sanook.com/web-fcgi-bin/web_search?country=hk&lang=zh_TW&search_input={}&sin=0&ein=30",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", UA)
        .header("Origin", "http://www.joox.com")
        .header("Referer", "http://www.joox.com")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    // joox 返回的单引号 JSON：把 ' 换成 " 再解析
    let json: serde_json::Value = serde_json::from_str(&text.replace('\'', "\"")).ok()?;
    let list = json.get("itemlist").and_then(|v| v.as_array())?;
    let out: Vec<SongInfo> = list.iter().filter_map(|s| {
        let songid = s.get("songid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if songid.is_empty() {
            return None;
        }
        let name = b64(s.get("info1").and_then(|v| v.as_str()).unwrap_or(""));
        if name.is_empty() {
            return None;
        }
        let artist = s.get("singer_list")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("name"))
            .and_then(|v| v.as_str())
            .map(b64)
            .unwrap_or_default();
        let album = s.get("info3").and_then(|v| v.as_str()).map(b64).unwrap_or_default();
        let duration = s.get("playtime").and_then(|v| v.as_u64());
        Some(SongInfo { songid, name, artist, album, duration })
    }).collect();
    if out.is_empty() { None } else { Some(out) }
}

/// 取播放 URL：web_get_songinfo 返回 JSONP，优先高清 r320Url，并把音质提级到 M800
async fn track(c: &MultiSourceClient, songid: &str) -> Option<String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    let url = format!(
        "http://api.joox.com/web-fcgi-bin/web_get_songinfo?songid={}&country=hk&lang=zh_cn&from_type=-1&channel_id=-1&_={}",
        songid, ts
    );
    let resp = c.http.get(&url)
        .header("User-Agent", UA)
        .header("Origin", "http://www.joox.com")
        .header("Referer", "http://www.joox.com")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let json = strip_callback(&text)?;
    for key in ["r320Url", "r192Url", "mp3Url", "m4aUrl"] {
        if let Some(u) = json.get(key).and_then(|v| v.as_str()) {
            if !u.is_empty() && u.starts_with("http") {
                return Some(upgrade_url(u));
            }
        }
    }
    None
}

/// 把 URL 里的 M<digit><digit>00 提升为 M800（M500/M600 → M800），提高音质
fn upgrade_url(u: &str) -> String {
    let b = u.as_bytes();
    for i in 0..b.len().saturating_sub(4) {
        if b[i] == b'M' && b[i + 1].is_ascii_digit() && b[i + 2].is_ascii_digit()
            && b[i + 3] == b'0' && b[i + 4] == b'0' {
            return format!("{}M800{}", &u[..i + 1], &u[i + 4..]);
        }
    }
    u.to_string()
}

/// 从 JSONP（如 MusicInfoCallback({...})）中剥离出 JSON 对象
fn strip_callback(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
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
