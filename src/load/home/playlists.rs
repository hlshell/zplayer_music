//! 首页「推荐歌单」板块 builder

use serde_json::Value;
use slint::SharedString;
use crate::core::song_util::{from, s2, s2s, to};
use super::PlaylistBase;

pub fn build_playlists(rp: &Result<Value, String>) -> Vec<PlaylistBase> {
    rp.as_ref()
        .ok()
        .and_then(|v| v.pointer("/result").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter().enumerate().map(|(i, p)| {
                let id = crate::core::ids::remember(0, p.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                let name = p.get("name").map(s2).unwrap_or_default();
                let plays = p.get("playCount").and_then(|v| v.as_u64()).map(fmt_plays).unwrap_or_default();
                let url = p.get("coverImgUrl").map(s2s)
                    .or_else(|| p.get("picUrl").map(s2s))
                    .unwrap_or_default();
                PlaylistBase { id, name, plays, from: from(i), to: to(i), url }
            }).collect()
        })
        .unwrap_or_default()
}

fn fmt_plays(n: u64) -> SharedString {
    let s = if n >= 100_000_000 {
        format!("{:.0}亿", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}万", n as f64 / 10_000.0)
    } else {
        n.to_string()
    };
    s.into()
}
