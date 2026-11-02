//! 酷我音乐音源（需先获取 kw_token cookie，无版权时 code=32007）

use super::{MultiSourceClient, ResolvedSong, SourceSong};

pub const NAME: &str = "kuwo";

struct SongInfo {
    rid: String,
    name: String,
    artist: String,
    album: String,
    cover: String,
    duration: Option<u64>,
}

/// 聚合搜索：返回该源歌曲条目列表
pub async fn search_songs(c: &MultiSourceClient, kw: &str) -> Option<Vec<SourceSong>> {
    let token = fetch_token(c).await?;
    let list = search_list(c, kw, &token).await?;
    Some(
        list.into_iter()
            .map(|s| SourceSong {
                source: NAME,
                id: s.rid,
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: s.cover,
                duration: s.duration,
            })
            .collect(),
    )
}

/// 按 id（rid）精确解析播放 URL
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    name: &str,
    artist: &str,
) -> Option<String> {
    let token = fetch_token(c).await?;
    let kw = if artist.is_empty() { name.to_string() } else { format!("{} {}", name, artist) };
    let list = search_list(c, &kw, &token).await?;
    let pick = list.into_iter().find(|s| s.rid == id)?;
    get_play_url(c, &pick.rid, &token).await
}

/// 搜索返回歌曲列表（含元数据）
async fn search_list(c: &MultiSourceClient, kw: &str, token: &str) -> Option<Vec<SongInfo>> {
    let url = format!(
        "https://www.kuwo.cn/api/www/search/searchMusicBykeyWord?key={}&pn=1&rn=20&httpsStatus=1",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Cookie", format!("kw_token={}", token))
        .header("csrf", token)
        .header("Referer", "https://www.kuwo.cn/search/list?key=")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("code").and_then(|v| v.as_i64()) != Some(200) {
        return None;
    }
    let list = json.pointer("/data/list").and_then(|v| v.as_array())?;
    let out: Vec<SongInfo> = list.iter().filter_map(|s| {
        let rid = s.get("rid").and_then(|v| v.as_i64())?.to_string();
        let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() { return None; }
        let artist = s.get("artist").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let album = s.get("album").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cover = s.get("albumPic").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let duration = s.get("duration").and_then(|v| v.as_u64());
        Some(SongInfo { rid, name, artist, album, cover, duration })
    }).collect();
    if out.is_empty() { None } else { Some(out) }
}

pub async fn try_resolve(c: &MultiSourceClient, title: &str, artist: &str) -> Option<ResolvedSong> {
    // 1. 获取 kw_token（首页 Set-Cookie）
    let token = fetch_token(c).await?;
    let kw = if artist.is_empty() { title.to_string() } else { format!("{} {}", title, artist) };

    // 2. 搜索（取第一个结果，携带时长用于匹配校验）
    let first = search_list(c, &kw, &token).await?.into_iter().next()?;

    // 3. 播放 URL
    let url = get_play_url(c, &first.rid, &token).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(first.rid),
        duration: first.duration,
    })
}

async fn fetch_token(c: &MultiSourceClient) -> Option<String> {
    let resp = c.http.get("https://www.kuwo.cn/")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send().await.ok()?;
    let set_cookies = resp.headers().get_all(reqwest::header::SET_COOKIE);
    for v in set_cookies.iter().filter_map(|v| v.to_str().ok()) {
        if let Some(rest) = v.strip_prefix("kw_token=") {
            if let Some(tok) = rest.split(';').next() {
                return Some(tok.to_string());
            }
        }
    }
    // cookie jar 里可能已存
    None
}

#[allow(dead_code)] // 备用：当前 search 路径未用此 rid 搜索，保留供未来兜底
async fn search_rid(c: &MultiSourceClient, kw: &str, token: &str) -> Option<String> {
    let url = format!(
        "https://www.kuwo.cn/api/www/search/searchMusicBykeyWord?key={}&pn=1&rn=20&httpsStatus=1",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Cookie", format!("kw_token={}", token))
        .header("csrf", token)
        .header("Referer", "https://www.kuwo.cn/search/list?key=")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("code").and_then(|v| v.as_i64()) != Some(200) {
        return None;
    }
    let list = json.pointer("/data/list").and_then(|v| v.as_array())?;
    for s in list {
        if let Some(rid) = s.get("rid").and_then(|v| v.as_i64()) {
            return Some(rid.to_string());
        }
    }
    None
}

async fn get_play_url(c: &MultiSourceClient, rid: &str, token: &str) -> Option<String> {
    let url = format!(
        "https://www.kuwo.cn/api/v1/www/music/playUrl?mid={}&type=music&httpsStatus=1",
        rid
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Cookie", format!("kw_token={}", token))
        .header("csrf", token)
        .header("Referer", "https://www.kuwo.cn/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("code").and_then(|v| v.as_i64()) != Some(200) {
        return None;
    }
    json.get("data").and_then(|d| d.get("url")).and_then(|v| v.as_str())
        .filter(|u| !u.is_empty() && u.starts_with("http"))
        .map(|s| s.to_string())
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
