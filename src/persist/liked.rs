//! 喜欢（收藏）数据：本地 liked.json 持久化 + 内存集合。
//! 歌曲收藏走既有 Vec<i64>；资源收藏（歌单/专辑/MV/电台）为 v2 新增，存快照避免列表拉网络。
//! 持久化跨启动真实保存；切换时由上层调网易云 like_song API（仅歌曲）。

use crate::AppWindow;
use std::sync::Mutex;

/// 收藏歌曲快照（含显示元数据，"我的最爱"页列表零网络展示）。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LikedSong {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    /// 封面 URL（收藏时快照）
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub duration_sec: f32,
    /// 收藏时间戳（秒）
    #[serde(default)]
    pub ts: i64,
}

/// 收藏的资源类型（kind 编码与详情页一致）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LikedResource {
    /// 0=歌单 1=专辑 3=电台 4=MV
    pub kind: i32,
    pub id: i64,
    pub name: String,
    /// 副标题（歌手/作者/简介一行）
    #[serde(default)]
    pub sub: String,
    /// 封面 URL（收藏时快照，列表显示零网络）
    #[serde(default)]
    pub cover: String,
    /// 收藏时间戳（秒）
    #[serde(default)]
    pub ts: i64,
}

/// v2 持久化结构。v1 为纯 Vec<i64>（仅歌曲 id），读取时兼容升级。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct LikedFile {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    songs: Vec<LikedSong>,
    #[serde(default)]
    resources: Vec<LikedResource>,
}

/// 已收藏歌曲集合（含元数据快照）。
static LIKED_SONGS: Mutex<Vec<LikedSong>> = Mutex::new(Vec::new());
/// 已收藏资源集合（v2 新增）。
static LIKED_RESOURCES: Mutex<Vec<LikedResource>> = Mutex::new(Vec::new());

pub(crate) fn load_liked() {
    let Some(s) = crate::platform::storage::read_text("liked.json") else { return; };
    // 兼容 v1：纯 id 数组即旧版格式（无元数据，列表仅显示 id，播放时走网络解析）
    if let Ok(v) = serde_json::from_str::<Vec<i64>>(&s) {
        *LIKED_SONGS.lock().unwrap() = v
            .into_iter()
            .map(|id| LikedSong { id, ..Default::default() })
            .collect();
        return;
    }
    if let Ok(f) = serde_json::from_str::<LikedFile>(&s) {
        *LIKED_SONGS.lock().unwrap() = f.songs;
        *LIKED_RESOURCES.lock().unwrap() = f.resources;
    }
}

pub(crate) fn save_liked() {
    let f = LikedFile {
        v: 2,
        songs: LIKED_SONGS.lock().unwrap().clone(),
        resources: LIKED_RESOURCES.lock().unwrap().clone(),
    };
    if let Ok(s) = serde_json::to_string(&f) {
        let _ = crate::platform::storage::write_text("liked.json", &s);
    }
}

// ============ 歌曲收藏（元数据快照）============

/// 查询歌曲是否已收藏（供 UI 同步与首页启动恢复使用）。
pub(crate) fn is_liked(uid: i64) -> bool {
    LIKED_SONGS.lock().map(|g| g.iter().any(|s| s.id == uid)).unwrap_or(false)
}

/// 切换歌曲收藏状态并持久化，返回切换后的状态（true=已收藏）。
/// meta 为收藏时的元数据快照（取消收藏时传 None，不会用到）。
pub(crate) fn toggle_liked(uid: i64, meta: Option<LikedSong>) -> bool {
    let mut g = LIKED_SONGS.lock().unwrap();
    let now = if let Some(pos) = g.iter().position(|s| s.id == uid) {
        g.remove(pos);
        false
    } else {
        let mut entry = meta.unwrap_or_default();
        entry.id = uid;
        entry.ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        g.push(entry);
        true
    };
    drop(g);
    save_liked();
    now
}

/// 读取全部收藏歌曲（按收藏时间倒序，供"我的最爱"页填充）。
pub(crate) fn list_songs() -> Vec<LikedSong> {
    let mut v = LIKED_SONGS.lock().unwrap().clone();
    v.sort_by(|a, b| b.ts.cmp(&a.ts));
    v
}

/// 缺少元数据的收藏 id（v1 旧格式升级，进入我的最爱页时后台批量补全一次）。
pub(crate) fn ids_without_meta() -> Vec<i64> {
    LIKED_SONGS
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.name.is_empty())
        .map(|s| s.id)
        .collect()
}

/// 按 id 批量补全收藏元数据并落盘（仅填空名条目，保留原收藏时间）。
pub(crate) fn update_song_meta(entries: Vec<LikedSong>) {
    let mut g = LIKED_SONGS.lock().unwrap();
    let mut changed = false;
    for e in entries {
        if let Some(s) = g.iter_mut().find(|s| s.id == e.id) {
            if s.name.is_empty() {
                let ts = s.ts;
                *s = LikedSong { ts, ..e };
                changed = true;
            }
        }
    }
    drop(g);
    if changed {
        save_liked();
    }
}

/// 按当前歌曲同步收藏状态显示（所有 set_current_song 之后调用）。
pub(crate) fn sync_liked(u: &AppWindow) {
    let uid = u.get_current_song().id as u32 as i64;
    u.set_liked(is_liked(uid));
}

// ============ 资源收藏（v2 新增）============

pub(crate) fn is_resource_liked(kind: i32, id: i64) -> bool {
    LIKED_RESOURCES
        .lock()
        .map(|g| g.iter().any(|r| r.kind == kind && r.id == id))
        .unwrap_or(false)
}

/// 切换资源收藏并持久化，返回切换后的状态（true=已收藏）。
pub(crate) fn toggle_resource(kind: i32, id: i64, name: String, sub: String, cover: String) -> bool {
    let mut g = LIKED_RESOURCES.lock().unwrap();
    let now = if let Some(pos) = g.iter().position(|r| r.kind == kind && r.id == id) {
        g.remove(pos);
        false
    } else {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        g.push(LikedResource { kind, id, name, sub, cover, ts });
        true
    };
    drop(g);
    save_liked();
    now
}

/// 按分类读取（kind 一致，倒序）。
pub(crate) fn list_resources_by_kind(kind: i32) -> Vec<LikedResource> {
    let mut v: Vec<LikedResource> = LIKED_RESOURCES
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.kind == kind)
        .cloned()
        .collect();
    v.sort_by(|a, b| b.ts.cmp(&a.ts));
    v
}
