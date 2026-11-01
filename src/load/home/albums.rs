//! 首页「新专辑」板块 builder

use serde_json::Value;
use slint::SharedString;
use crate::core::song_util::{from, s2, s2s, to};
use super::AlbumBase;

pub fn build_albums(na: &Result<Value, String>) -> Vec<AlbumBase> {
    na.as_ref()
        .ok()
        .and_then(|v| v.pointer("/albums").and_then(|a| a.as_array()))
        .map(|arr| {
            arr.iter().enumerate().take(5).map(|(i, al)| {
                let id = crate::core::ids::remember(1, al.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                let name = al.get("name").map(s2).unwrap_or_default();
                let artist = al.get("artist").and_then(|x| x.get("name")).map(s2).unwrap_or_default();
                let year = al.get("publishTime").and_then(|v| v.as_i64())
                    .map(ms_to_year).unwrap_or_default();
                let url = al.get("picUrl").map(s2s).unwrap_or_default();
                AlbumBase { id, name, artist, year, from: from(i), to: to(i), url }
            }).collect()
        })
        .unwrap_or_default()
}

fn ms_to_year(ms: i64) -> SharedString {
    let secs = ms / 1000;
    let days = secs / 86400;
    let year = 1970 + (days as f64 / 365.25) as i64;
    year.to_string().into()
}
