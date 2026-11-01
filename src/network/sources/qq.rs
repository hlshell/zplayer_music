//! QQ 音乐音源（搜索无签名；播放走 fcg_musicexpress vkey 老接口，VIP/无版权可能返回空）

use super::{MultiSourceClient, ResolvedSong, SourceSong};

pub const NAME: &str = "qq";

struct SongInfo {
    songmid: String,
    name: String,
    artist: String,
    album: String,
    cover: String,
    duration: Option<u64>,
}

/// 聚合搜索：返回该源歌曲条目列表
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let songs = search(c, kw).await?;
    Some(
        songs
            .into_iter()
            .map(|s| SourceSong {
                source: NAME,
                id: s.songmid,
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: s.cover,
                duration: s.duration,
            })
            .collect(),
    )
}

/// 按 id（songmid）精确解析播放 URL
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    name: &str,
    artist: &str,
) -> Option<String> {
    let kw = if artist.is_empty() { name.to_string() } else { format!("{} {}", name, artist) };
    let songs = search(c, &kw).await?;
    let pick = songs.into_iter().find(|s| s.songmid == id)?;
    get_play_url(c, &pick.songmid).await
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

    let url = get_play_url(c, &pick.songmid).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(pick.songmid),
        duration: pick.duration,
    })
}

async fn search(c: &MultiSourceClient, kw: &str) -> Option<Vec<SongInfo>> {
    let url = format!(
        "https://c.y.qq.com/soso/fcgi-bin/client_search_cp?w={}&p=1&n=20&format=json&cr=1&aggr=1&t=0",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Referer", "https://y.qq.com/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("code").and_then(|v| v.as_i64()) != Some(0) {
        return None;
    }
    let list = json.pointer("/data/song/list").and_then(|v| v.as_array())?;
    let out: Vec<SongInfo> = list.iter().filter_map(|s| {
        let songmid = s.get("songmid").and_then(|v| v.as_str())?.to_string();
        let name = s.get("songname").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() { return None; }
        let artist = s.get("singer")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|x| x.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let album = s.get("albumname").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let albummid = s.get("albummid").and_then(|v| v.as_str()).unwrap_or("");
        let cover = if albummid.is_empty() { String::new() } else { format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{}.jpg", albummid) };
        let duration = s.get("interval")
            .and_then(|v| v.as_u64())
            .or_else(|| s.get("interval").and_then(|v| v.as_str()).and_then(|x| x.trim().parse().ok()));
        Some(SongInfo { songmid, name, artist, album, cover, duration })
    }).collect();
    if out.is_empty() { None } else { Some(out) }
}

async fn get_play_url(c: &MultiSourceClient, songmid: &str) -> Option<String> {
    let guid = random_guid();
    let filename = format!("C400{}.m4a", songmid);
    let url = format!(
        "https://c.y.qq.com/base/fcgi-bin/fcg_musicexpress.fcg?json=3&guid={}&format=json&loginUin=0&cid=205361747&callback=cb&uin=0&songmid={}&filename={}&songtype=0",
        guid, songmid, filename
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Referer", "https://y.qq.com/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    let json = strip_jsonp(&text)?;
    let data = json.get("data").and_then(|v| v.as_array())?;
    for item in data {
        let obj = item.as_object()?;
        for (_, v) in obj {
            if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
                if !u.is_empty() && u.starts_with("http") {
                    return Some(u.to_string());
                }
            }
        }
    }
    None
}

fn strip_jsonp(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

fn random_guid() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..10).map(|_| rng.gen_range(b'0'..b'9') as char).collect()
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
