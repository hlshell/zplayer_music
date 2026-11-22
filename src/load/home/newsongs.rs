//! 首页「新歌速递」板块 builder

use serde_json::Value;
use crate::core::song_util::{fmt_dur, song_cover_url, song_fields, SongBase};

pub fn build_newsongs(ns: &Result<Value, String>) -> Vec<SongBase> {
    ns.as_ref()
        .ok()
        .and_then(|v| v.pointer("/result").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter().map(|n| {
                let (id, name, artist, album, dt) = song_fields(n);
                SongBase {
                    id,
                    name,
                    artist,
                    album,
                    duration: fmt_dur(dt),
                    duration_sec: dt as f32,
                    url: song_cover_url(n),
                }
            }).collect()
        })
        .unwrap_or_default()
}
