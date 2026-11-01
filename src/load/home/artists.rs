//! 首页「歌手」板块 builder

use serde_json::Value;
use crate::core::song_util::{from, s2, s2s, to};
use super::ArtistBase;

pub fn build_artists(ta: &Result<Value, String>) -> Vec<ArtistBase> {
    ta.as_ref()
        .ok()
        .and_then(|v| v.pointer("/artists").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter().enumerate().map(|(i, a)| {
                let id = crate::core::ids::remember(2, a.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                let name = a.get("name").map(s2).unwrap_or_default();
                let initial = name.chars().next().map(|c| c.to_string()).unwrap_or_default().into();
                let url = a.get("picUrl").map(s2s)
                    .or_else(|| a.get("img1v1Url").map(s2s))
                    .unwrap_or_default();
                ArtistBase { id, name, initial, from: from(i), to: to(i), url }
            }).collect()
        })
        .unwrap_or_default()
}
