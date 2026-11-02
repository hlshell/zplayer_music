//! 当前歌曲歌词：内存 (秒, 文本) 列表 + LRC 解析。
//! 由播放进度回调读取 LYRICS 定位高亮行；播放时写入解析结果。

use std::sync::Mutex;

/// 当前歌曲歌词（秒, 文本），供播放进度定位高亮行。
pub static LYRICS: Mutex<Vec<(f32, String)>> = Mutex::new(Vec::new());

/// 由 (秒, 文本) 列表构造 LyricLine 模型（桌面/web 共用）。
/// 无逐词时间时整行作为一个 LyricWord（start=行时间，end=下一行时间，
/// 末行 end=start+5s），逐词进度即为行内线性填充。
/// 注意：LyricLine 含 ModelRc（非 Send），必须在 UI 线程构造。
pub fn build_lyric_lines(parsed: &[(f32, String)]) -> Vec<crate::LyricLine> {
    let n = parsed.len();
    parsed
        .iter()
        .enumerate()
        .map(|(i, (t, text))| {
            let start = (*t * 1000.0) as i32;
            let end = if i + 1 < n {
                (parsed[i + 1].0 * 1000.0) as i32
            } else {
                start + 5000
            };
            crate::LyricLine {
                words: std::rc::Rc::new(slint::VecModel::from(vec![crate::LyricWord {
                    text: text.as_str().into(),
                    start_ms: start,
                    end_ms: end,
                }]))
                .into(),
                start_ms: start,
                end_ms: end,
            }
        })
        .collect()
}

/// 解析 LRC 歌词为 (秒, 文本) 列表。
pub(crate) fn parse_lrc(raw: &str) -> Vec<(f32, String)> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let mut tags = Vec::new();
        let mut rest = line;
        while let Some(open) = rest.find('[') {
            if let Some(close) = rest[open..].find(']') {
                tags.push(rest[open + 1..open + close].to_string());
                rest = &rest[open + close + 1..];
            } else {
                break;
            }
        }
        let text = rest.trim().to_string();
        for tag in tags {
            let parts: Vec<&str> = tag.split(':').collect();
            if parts.len() != 2 { continue; }
            let mm: f32 = parts[0].parse().unwrap_or(0.0);
            let sec: f32 = parts[1].parse().unwrap_or(0.0);
            out.push((mm * 60.0 + sec, text.clone()));
        }
    }
    // 按时间升序：LRC 多时间戳行可能乱序，排序同时保证
    // ① 进度定位可用二分（见 handlers/player/play.rs）② 逐词时长按相邻行推导正确
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}
