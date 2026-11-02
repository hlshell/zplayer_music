//! 首页「新歌速递」板块 builder

use serde_json::Value;
use crate::core::song_util::{s2, song_cover_url};
use super::NewSongBase;

pub fn build_newsongs(ns: &Result<Value, String>) -> Vec<NewSongBase> {
    ns.as_ref()
        .ok()
        .and_then(|v| v.pointer("/result").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter().map(|n| {
                let id = n.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let name = n.get("name").map(s2).unwrap_or_default();
                let artist = n.get("song")
                    .and_then(|s| s.get("artists"))
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|x| x.get("name"))
                    .map(s2)
                    .unwrap_or_default();
                NewSongBase { id, name, artist, url: song_cover_url(n) }
            }).collect()
        })
        .unwrap_or_default()
}
