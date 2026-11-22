//! 当前歌曲歌词：内存 (秒, 文本) 列表 + LRC 解析。
//! 由播放进度回调读取 LYRICS 定位高亮行；播放时写入解析结果。

use std::sync::Mutex;

/// 当前歌曲歌词（秒, 文本），供播放进度定位高亮行。
pub static LYRICS: Mutex<Vec<(f32, String)>> = Mutex::new(Vec::new());

/// 由 (秒, 文本) 列表构造 LyricLine 模型（桌面/web 共用）。
///
/// LRC 只有整行时间，没有逐字时间，因此把整行切成「逐字单元」并按字宽
/// 均摊行时长：每个单元一个 LyricWord，UI 端逐单元填充 —— 扫到哪个字，
/// 哪个字才点亮（而不是整行连续渐变）。
/// 注意：LyricLine 含 ModelRc（非 Send），必须在 UI 线程构造。
#[cfg(feature = "desktop")]
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
                text: text.as_str().into(),
                words: std::rc::Rc::new(slint::VecModel::from(split_words(text, start, end)))
                    .into(),
                start_ms: start,
                end_ms: end,
            }
        })
        .collect()
}

/// 把一行切分成逐字单元，并按「显示宽度」均摊 [start, end] 时长。
/// 规则：CJK/全角字符各自成单元；连续的半角字母数字按空白成词；空白并入前一个单元，
/// 保证所有单元拼回去与原文本一致（排版不变）。
#[cfg(feature = "desktop")]
fn split_words(text: &str, start: i32, end: i32) -> Vec<crate::LyricWord> {
    let mut units: Vec<String> = Vec::new();
    let mut latin = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if latin.is_empty() {
                // 词间空白：并入前一个单元，保持原排版
                if let Some(last) = units.last_mut() {
                    last.push(c);
                    continue;
                }
            }
            latin.push(c);
        } else if is_wide(c) {
            if !latin.is_empty() {
                units.push(std::mem::take(&mut latin));
            }
            units.push(c.to_string());
        } else {
            latin.push(c);
        }
    }
    if !latin.is_empty() {
        units.push(latin);
    }
    if units.is_empty() {
        return Vec::new();
    }

    let weight = |s: &str| -> f32 {
        s.chars()
            .map(|c| if is_wide(c) { 2.0 } else { 1.0 })
            .sum::<f32>()
    };
    let total: f32 = units.iter().map(|u| weight(u)).sum::<f32>().max(1.0);
    let full = (end - start).max(1) as f32;
    // 预留收尾：最后一个字要在整行结束前就点亮完成。
    // 进度每 250ms 才刷新一次，若最后一字的结束时间正好等于行尾，
    // 往往停在最后一字的中途就切到下一行，看起来“最后一个字只亮一半”。
    let tail = (full * 0.08).min(350.0);
    let span = (full - tail).max(1.0);

    let mut out = Vec::with_capacity(units.len());
    let mut acc = 0.0;
    let mut cursor = start;
    for u in units {
        acc += weight(&u);
        let w_end = start + (span * acc / total) as i32;
        out.push(crate::LyricWord {
            text: u.as_str().into(),
            start_ms: cursor,
            end_ms: w_end.max(cursor + 1),
        });
        cursor = w_end;
    }
    out
}

/// 是否宽字符（CJK/假名/谚文/全角标点），用于按显示宽度分配时间。
#[cfg(feature = "desktop")]
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x20000..=0x3FFFD
    )
}

/// 解析 LRC 歌词为 (秒, 文本) 列表。
/// 仅接受 `mm:ss[.xx]` 两段式数字时间标签；`[ti:]`/`[ar:]`/`[al:]`/`[by:]`/`[offset:]`
/// 等元数据标签一律忽略（否则会被当成 0 秒空行）；空文本行不入表。
pub(crate) fn parse_lrc(raw: &str) -> Vec<(f32, String)> {
    let mut out: Vec<(f32, String)> = Vec::new();
    for line in raw.lines() {
        let mut rest = line;
        let mut times: Vec<f32> = Vec::new();
        while let Some(open) = rest.find('[') {
            if let Some(close_off) = rest[open..].find(']') {
                let tag = rest[open + 1..open + close_off].trim();
                rest = &rest[open + close_off + 1..];
                // 仅接受 mm:ss[.xx] 两段式数字时间标签
                let mut parts = tag.split(':');
                if let (Some(mm_s), Some(ss_s), None) = (parts.next(), parts.next(), parts.next()) {
                    if let (Ok(mm), Ok(ss)) =
                        (mm_s.trim().parse::<f32>(), ss_s.trim().parse::<f32>())
                    {
                        times.push(mm * 60.0 + ss);
                    }
                }
            } else {
                break;
            }
        }
        let text = rest.trim().to_string();
        // 空文本行（纯时间戳/间奏）不占位，避免出现空白行
        if text.is_empty() {
            continue;
        }
        for t in times {
            out.push((t, text.clone()));
        }
    }
    // 按时间升序：LRC 多时间戳行可能乱序，排序同时保证
    // ① 进度定位可用二分（见 handlers/player/play.rs）② 逐词时长按相邻行推导正确
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}
