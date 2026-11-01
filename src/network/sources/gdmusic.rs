//! gdmusic（GD音乐台）兜底音源：调用第三方聚合服务
//! `https://music-api.gdstudio.xyz/api.php`（joox / tidal / netease 中转）。
//! 从 old 的 gdmusic.ts 移植：先搜索前 5 条 → 用「歌名 + 歌手」归一化匹配
//! 挑出与原曲相符的候选（避免版权下架歌解析成翻唱/同名"货不对版"）→ 再取直链。

use super::{MultiSourceClient, ResolvedSong};
use serde_json::Value;

pub const NAME: &str = "gdmusic";

const BASE: &str = "https://music-api.gdstudio.xyz/api.php";

/// 归一化文本：去括号备注、空白、常见标点，转小写。
/// 整个字符串都在括号里时退化为仅去标点，避免归一化成空串。
fn normalize_text(text: &str) -> String {
    let stripped = strip_paren(text)
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && !PUNCT.contains(c))
        .collect::<String>();
    if stripped.is_empty() {
        text.to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace() && !PUNCT.contains(c))
            .collect()
    } else {
        stripped
    }
}

const PUNCT: &[char] = &[
    '-', '_', '—', '·', '・', '\'', '"', '‘', '’', '“', '”', '!', '！', '?', '？', '.', ',', '，', '。', '&', '＆', '+',
];

/// 去掉 `（(【[ ... ）)】]` 括号内内容（Live/翻唱/伴奏等备注）。
fn strip_paren(text: &str) -> String {
    let open = ['（', '(', '【', '['];
    let close = ['）', ')', '】', ']'];
    let mut depth = 0usize;
    let mut out = String::new();
    for c in text.chars() {
        if open.contains(&c) {
            depth += 1;
            continue;
        }
        if close.contains(&c) {
            if depth > 0 {
                depth -= 1;
            }
            continue;
        }
        if depth == 0 {
            out.push(c);
        }
    }
    if out.trim().is_empty() {
        out = text.chars().filter(|c| !c.is_whitespace()).collect();
    }
    out
}

fn is_name_matched(expected: &str, candidate: &str) -> bool {
    let e = normalize_text(expected);
    let c = normalize_text(candidate);
    if e.is_empty() || c.is_empty() {
        return false;
    }
    e == c || c.contains(&e) || e.contains(&c)
}

/// 候选的歌手文本（artist 可能是字符串 / 数组 / 数组对象）。
fn get_candidate_artist(artist: &Value) -> String {
    match artist {
        Value::Array(a) => a
            .iter()
            .filter_map(|i| {
                if let Some(s) = i.as_str() {
                    Some(s.to_string())
                } else {
                    i.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// 歌名必须匹配；候选带歌手时歌手也必须匹配，宁可解析失败也不返回错误的歌。
fn pick_best(candidates: &[Value], expected_name: &str, expected_artists: &[String]) -> Option<Value> {
    let mut best: Option<Value> = None;
    let mut best_score = 0i32;
    for item in candidates {
        let Some(_) = item.get("id") else { continue };
        let cand_name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if !is_name_matched(expected_name, cand_name) {
            continue;
        }
        let cand_artist = normalize_text(&get_candidate_artist(item.get("artist").unwrap_or(&Value::Null)));
        let score: i32 = if expected_artists.is_empty() {
            2
        } else if cand_artist.is_empty() {
            1
        } else {
            let matched = expected_artists.iter().any(|name| {
                let normalized = normalize_text(name);
                !normalized.is_empty()
                    && (cand_artist.contains(&normalized) || normalized.contains(&cand_artist))
            });
            if !matched {
                continue;
            }
            3
        };
        if score > best_score {
            best = Some(item.clone());
            best_score = score;
        }
    }
    best
}

/// Value 转字符串 id（可能是数字或字符串）。
fn val_str(v: &Value) -> String {
    v.as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| v.as_i64().map(|n| n.to_string()).unwrap_or_default())
}

/// 简单 URL 编码（UTF-8 非 ASCII 逐字节 %XX）。
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 拆分歌手（`、` / `&` / `，` 分隔）为数组，供匹配校验。
fn split_artists(s: &str) -> Vec<String> {
    s.split(['、', '&', '＆', ',', '，'])
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

/// 兜底解析：逐源 joox → tidal → netease 搜索并校验，返回原曲相符的可播 URL。
pub async fn try_resolve(c: &MultiSourceClient, name: &str, artist: &str) -> Option<ResolvedSong> {
    let query = format!("{} {}", name, artist).trim().to_string();
    if query.chars().count() < 2 {
        return None;
    }
    let artists = split_artists(artist);
    for source in ["joox", "tidal", "netease"] {
        let search = format!(
            "{BASE}?types=search&source={source}&name={}&count=5&pages=1",
            urlencode(&query)
        );
        let Ok(resp) = c.http.get(&search).send().await else { continue };
        let Ok(v) = resp.json::<Value>().await else { continue };
        let Some(arr) = v.as_array() else { continue };
        if arr.is_empty() {
            continue;
        }
        let Some(best) = pick_best(arr, name, &artists) else { continue };
        let id = best.get("id").map(val_str).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let track_source = best.get("source").and_then(|s| s.as_str()).unwrap_or(source);
        let url_req = format!("{BASE}?types=url&source={track_source}&id={id}&br=999");
        let Ok(ur) = c.http.get(&url_req).send().await else { continue };
        let Ok(uv) = ur.json::<Value>().await else { continue };
        let Some(url) = uv.get("url").and_then(|u| u.as_str()) else { continue };
        if url.is_empty() {
            continue;
        }
        return Some(ResolvedSong {
            url: url.to_string(),
            source: NAME,
            name: name.to_string(),
            artist: artist.to_string(),
            platform_id: Some(id),
            duration: None,
        });
    }
    None
}
