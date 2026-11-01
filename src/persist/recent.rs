//! 最近播放记录：本地 recent.json 持久化 + 内存集合。
//! 记录每次播放过的歌曲（去重按 id，最新在前），支持单条删除与清空。

use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecentEntry {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub duration_sec: f32,
    /// 资源类型：0=歌曲，4=MV（与 liked kind 编码对齐）
    #[serde(default)]
    pub kind: i32,
    #[serde(default)]
    pub ts: i64,
}

/// 上限保留 200 条。
const MAX: usize = 200;

static RECENT: Mutex<Vec<RecentEntry>> = Mutex::new(Vec::new());

pub(crate) fn load_recent() {
    let Some(s) = crate::platform::storage::read_text("recent.json") else { return; };
    if let Ok(v) = serde_json::from_str::<Vec<RecentEntry>>(&s) {
        // 按 id 去重（保留首条 = 最新），清理历史版本可能残留的重复条目
        let mut seen = std::collections::HashSet::new();
        let v: Vec<RecentEntry> = v.into_iter().filter(|e| seen.insert(e.id)).collect();
        *RECENT.lock().unwrap() = v;
    }
}

pub(crate) fn save_recent() {
    if let Ok(s) = serde_json::to_string(&*RECENT.lock().unwrap()) {
        let _ = crate::platform::storage::write_text("recent.json", &s);
    }
}

/// 记录一次播放（id 去重并前置）。kind: 0=歌曲, 4=MV。
#[allow(clippy::too_many_arguments)]
pub(crate) fn record(id: i64, name: String, artist: String, album: String, cover: String, duration: String, duration_sec: f32, kind: i32) {
    if id == 0 { return; }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut g = RECENT.lock().unwrap();
    if let Some(pos) = g.iter().position(|e| e.id == id && e.kind == kind) {
        g.remove(pos);
    }
    g.insert(0, RecentEntry { id, name, artist, album, cover, duration, duration_sec, kind, ts });
    g.truncate(MAX);
    drop(g);
    save_recent();
}

/// 按分类读取（kind 一致，倒序）。
pub(crate) fn list_by_kind(kind: i32) -> Vec<RecentEntry> {
    RECENT.lock().unwrap().iter().filter(|e| e.kind == kind).cloned().collect()
}

pub(crate) fn remove(id: i64) {
    let mut g = RECENT.lock().unwrap();
    if let Some(pos) = g.iter().position(|e| e.id == id) {
        g.remove(pos);
        drop(g);
        save_recent();
    }
}

pub(crate) fn clear() {
    RECENT.lock().unwrap().clear();
    save_recent();
}
