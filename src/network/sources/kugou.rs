//! 酷狗音乐音源（需 md5 签名）

use super::md5;
use super::{MultiSourceClient, ResolvedSong, SourceSong};

pub const NAME: &str = "kugou";

const APPKEY: &str = "NVPh5oo715z5DIWAeQlhMDgsW6fjKLJt06OYynYq6e8P1BYQVve";

struct SongInfo {
    hash: String,
    album_id: String,
    album_audio_id: String,
    track_id: String,
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
                id: s.hash,
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: s.cover,
                duration: s.duration,
            })
            .collect(),
    )
}

/// 按 id（FileHash）精确解析播放 URL
pub async fn resolve_url_by_id(
    c: &MultiSourceClient,
    id: &str,
    name: &str,
    artist: &str,
) -> Option<String> {
    let kw = if artist.is_empty() { name.to_string() } else { format!("{} {}", name, artist) };
    let songs = search(c, &kw).await?;
    let pick = songs.into_iter().find(|s| s.hash == id)?;
    get_play_url(c, &pick).await
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

    let url = get_play_url(c, &pick).await?;
    if url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url,
        source: NAME,
        name: title.to_string(),
        artist: artist.to_string(),
        platform_id: Some(pick.hash),
        duration: pick.duration,
    })
}

async fn search(c: &MultiSourceClient, kw: &str) -> Option<Vec<SongInfo>> {
    let url = format!(
        "https://complexsearch.kugou.com/v2/search/song?keyword={}&page=1&pagesize=20&platform=WebFilter&srcappid=2919&clientver=2000&tag=em&filter=10&iscorrection=1&privilege_filter=0",
        urlencoding(kw)
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Referer", "https://www.kugou.com/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("status").and_then(|v| v.as_i64()) != Some(1) {
        return None;
    }
    let lists = json.pointer("/data/lists").and_then(|v| v.as_array())?;
    let out: Vec<SongInfo> = lists.iter().filter_map(|s| {
        let hash = s.get("FileHash").and_then(|v| v.as_str())?.to_string();
        let name = s.get("SongName").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if name.is_empty() { return None; }
        let artist = s.get("SingerName").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let album_id = s.get("album_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let album = s.get("AlbumName").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cover = s.get("Img").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let duration = s.get("duration")
            .and_then(|v| v.as_u64())
            .or_else(|| s.get("duration").and_then(|v| v.as_str()).and_then(|x| x.trim().parse().ok()));

        let (album_audio_id, track_id) = s.get("audioInfo")
            .and_then(|v| v.as_str())
            .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
            .map(|info| {
                let aid = info.get("album_audio_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let tid = info.get("track_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                (aid, tid)
            })
            .unwrap_or_default();

        Some(SongInfo { hash, album_id, album_audio_id, track_id, name, artist, album, cover, duration })
    }).collect();
    if out.is_empty() { None } else { Some(out) }
}

async fn get_play_url(c: &MultiSourceClient, s: &SongInfo) -> Option<String> {
    // 在块内生成随机串，结束于 await 之前（ThreadRng 非 Send）
    let (dfid, mid) = {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let dfid: String = (0..24).map(|_| {
            let i: u8 = rng.gen_range(0..36);
            if i < 10 { (b'0' + i) as char } else { (b'a' + (i - 10)) as char }
        }).collect();
        let mid: String = (0..32).map(|_| {
            let i: u8 = rng.gen_range(0..16);
            format!("{:x}", i)
        }).collect();
        (dfid, mid)
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).ok()?.as_secs().to_string();
    let appid = "1014";
    let platid = "4";
    let typ = "1";

    // signature = md5(key + hash + kg_mid + appid + platid + dfid + timestamp + album_audio_id + album_audio_id + track_id + type)
    let sig_src = format!(
        "{}{}{}{}{}{}{}{}{}{}{}",
        APPKEY, s.hash, mid, appid, platid, dfid, ts, s.album_audio_id, s.album_audio_id, s.track_id, typ
    );
    let signature = md5::md5(&sig_src);

    let url = format!(
        "https://wwwapi.kugou.com/yy/index.php?r=play/getdata&hash={}&dfid={}&appid={}&mid={}&platid={}&album_id={}&_={}&signature={}",
        s.hash, dfid, appid, mid, platid, s.album_id, ts, signature
    );
    let resp = c.http.get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .header("Referer", "https://www.kugou.com/")
        .send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    if json.get("status").and_then(|v| v.as_i64()) != Some(1) {
        return None;
    }
    json.pointer("/data/play_url").and_then(|v| v.as_str())
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
