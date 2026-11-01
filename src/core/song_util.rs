//! 通用歌曲解析工具 + 渐变配色盘 + 模型构造。
//! 供 home 各板块 builder 与 main 播放/详情逻辑共用，无 crate 级业务依赖。

use serde_json::Value;
use slint::{ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// 歌曲基础数据（跨模块共享：HERO_POOL 供播放全部/详情页读取；解析器产出）
#[derive(Clone)]
pub struct SongBase {
    pub id: i32,
    pub name: SharedString,
    pub artist: SharedString,
    pub album: SharedString,
    pub duration: SharedString,
    pub duration_sec: f32,
    pub url: String,
}

// 渐变配色盘（from, to）——对齐前端紫青主题，按索引轮换
const PALETTE: &[(u32, u32)] = &[
    (0x7c6bff, 0x22d3ee),
    (0xf59e0b, 0xef4444),
    (0x22c55e, 0x14b8a6),
    (0x8b5cf6, 0xec4899),
    (0x0ea5e9, 0x6366f1),
    (0xf472b6, 0xa78bfa),
    (0xfcd34d, 0xf59e0b),
    (0xa5b4fc, 0x818cf8),
    (0xf0abfc, 0xe879f9),
    (0x34d399, 0x0ea5e9),
    (0xfed7aa, 0xfb923c),
    (0xa7f3d0, 0x34d399),
];

pub fn from(i: usize) -> slint::Color {
    let (c, _) = PALETTE[i % PALETTE.len()];
    slint::Color::from_argb_u8(0xFF, ((c >> 16) & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c & 0xFF) as u8)
}

pub fn to(i: usize) -> slint::Color {
    let (_, c) = PALETTE[i % PALETTE.len()];
    slint::Color::from_argb_u8(0xFF, ((c >> 16) & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, (c & 0xFF) as u8)
}

pub(crate) fn fmt_dur(sec: u64) -> SharedString {
    format!("{:02}:{:02}", sec / 60, sec % 60).into()
}

/// 通用歌曲数组解析为 SongBase（详情页用：歌单/专辑/歌手响应中的 tracks/songs）。
/// url 字段存该曲封面 URL（与 HERO_POOL 约定一致，供封面缓存/补图复用）。
pub fn parse_tracks(arr: &[Value]) -> Vec<SongBase> {
    arr.iter().map(|s| {
        let (id, name, artist, album, dt) = song_fields(s);
        SongBase {
            id,
            name,
            artist,
            album,
            duration: fmt_dur(dt),
            duration_sec: dt as f32,
            url: song_cover_url(s),
        }
    }).collect()
}

pub(crate) fn s2(v: &Value) -> SharedString {
    v.as_str().unwrap_or("").into()
}

pub(crate) fn s2s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// 从歌曲对象提取封面 URL（标准 al.picUrl / 旧版顶层 album.picUrl / 新歌 song.album.picUrl）
pub(crate) fn song_cover_url(s: &Value) -> String {
    s.get("al").and_then(|x| x.get("picUrl")).map(s2s)
        .or_else(|| s.get("album").and_then(|x| x.get("picUrl")).map(s2s))
        .or_else(|| {
            s.get("song")
                .and_then(|x| x.get("album"))
                .and_then(|x| x.get("picUrl"))
                .map(s2s)
        })
        .unwrap_or_default()
}

/// 从歌曲对象提取 (id, name, artist, album, duration_sec)
/// 兼容三种形状：v3 榜单 track（ar/al/dt）、旧版顶层（artists/album/duration）、
/// 新歌速递（song.artists / song.album / song.duration）。
pub(crate) fn song_fields(s: &Value) -> (i32, SharedString, SharedString, SharedString, u64) {
    let id = s.get("id").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let name = s.get("name").map(s2).unwrap_or_default();
    // 歌手：ar[0].name → artists[0].name → song.artists[0].name
    let artist = s.get("ar")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("name"))
        .map(s2)
        .or_else(|| {
            s.get("artists")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("name"))
                .map(s2)
        })
        .or_else(|| {
            s.get("song")
                .and_then(|x| x.get("artists"))
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("name"))
                .map(s2)
        })
        .unwrap_or_default();
    // 专辑：al.name → album.name → song.album.name
    let album = s.get("al").and_then(|x| x.get("name")).map(s2)
        .or_else(|| s.get("album").and_then(|x| x.get("name")).map(s2))
        .or_else(|| s.get("song").and_then(|x| x.get("album")).and_then(|x| x.get("name")).map(s2))
        .unwrap_or_default();
    // 时长（毫秒→秒）：dt → duration → song.duration
    let dt = s.get("dt")
        .or_else(|| s.get("duration"))
        .or_else(|| s.get("song").and_then(|x| x.get("duration")))
        .and_then(|v| v.as_u64()).unwrap_or(0) / 1000;
    (id, name, artist, album, dt)
}

/// 把 Vec 包装成 Slint 模型（供 UI 线程填充模型用）
pub fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(items)).into()
}

/// 播放量格式化（歌单网格角标）：亿/万/原数，对齐 home 板块 fmt_plays。
pub(crate) fn fmt_plays_cat(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.0}亿", n as f64 / 100_000_000.0)
    } else if n >= 10_000 {
        format!("{:.1}万", n as f64 / 10_000.0)
    } else {
        n.to_string()
    }
}
