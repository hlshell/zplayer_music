//! 播放队列持久化：queue.json 保存队列条目、当前曲目与播放进度。
//! 移动端重启后恢复上次队列；桌面/Web 不写入该文件，初始化行为不受影响。

use std::sync::Mutex;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueueEntry {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    /// 封面 URL（同时作为缓存键）
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub duration_sec: f32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct QueueState {
    #[serde(default)]
    pub items: Vec<QueueEntry>,
    /// 当前曲目 id；0 表示未指定
    #[serde(default)]
    pub current: i64,
    /// 播放进度比例 0~1
    #[serde(default)]
    pub progress: f32,
}

const FILE: &str = "queue.json";
/// 单曲队列上限：歌单详情最大 500 首，超出截断避免文件过大
const MAX: usize = 500;

static STATE: Mutex<Option<QueueState>> = Mutex::new(None);

/// 启动时读取磁盘快照（无文件则保持空）。
pub(crate) fn init() {
    let Some(s) = crate::platform::storage::read_text(FILE) else { return; };
    if let Ok(mut st) = serde_json::from_str::<QueueState>(&s) {
        st.items.truncate(MAX);
        if !st.items.is_empty() {
            *STATE.lock().unwrap() = Some(st);
        }
    }
}

/// 是否存在可恢复的队列（供首页数据加载判断是否覆盖队列）。
pub(crate) fn has_saved() -> bool {
    STATE.lock().unwrap().as_ref().map_or(false, |s| !s.items.is_empty())
}

pub(crate) fn state() -> Option<QueueState> {
    STATE.lock().unwrap().clone()
}

/// 保存快照：先更新内存，再落盘。调用方负责在切歌/暂停/定时等低频率时机触发。
pub(crate) fn save(items: Vec<QueueEntry>, current: i64, progress: f32) {
    let mut items = items;
    items.truncate(MAX);
    let st = QueueState {
        items,
        current,
        progress: progress.clamp(0.0, 1.0),
    };
    if let Ok(s) = serde_json::to_string(&st) {
        let _ = crate::platform::storage::write_text(FILE, &s);
    }
    *STATE.lock().unwrap() = Some(st);
}
