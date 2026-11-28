//! 坏歌标记：播放失败（无法获取音频/解码失败/流中断）的歌曲 id 持久化。
//! 循环/随机播放遇到已标记歌曲自动跳过；用户再次点击该曲时先清除标记再重试。

use std::sync::Mutex;

const FILE: &str = "bad_songs.json";

static IDS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// 启动时读取磁盘快照（去重）。
pub(crate) fn load() {
    let Some(s) = crate::platform::storage::read_text(FILE) else { return; };
    if let Ok(v) = serde_json::from_str::<Vec<i64>>(&s) {
        let mut seen = std::collections::HashSet::new();
        *IDS.lock().unwrap() = v.into_iter().filter(|id| *id != 0 && seen.insert(*id)).collect();
    }
}

fn save() {
    if let Ok(s) = serde_json::to_string(&*IDS.lock().unwrap()) {
        let _ = crate::platform::storage::write_text(FILE, &s);
    }
}

/// 该歌曲是否已被标记为无法播放。
pub(crate) fn is_bad(id: i64) -> bool {
    IDS.lock().map(|g| g.contains(&id)).unwrap_or(false)
}

/// 全部坏歌 id（供 UI 列表标示）。
pub(crate) fn ids() -> Vec<i64> {
    IDS.lock().map(|g| g.clone()).unwrap_or_default()
}

/// 标记为无法播放（去重落盘）。
pub(crate) fn mark(id: i64) {
    if id == 0 {
        return;
    }
    let inserted = {
        let mut g = IDS.lock().unwrap();
        if g.contains(&id) {
            false
        } else {
            g.push(id);
            true
        }
    };
    if inserted {
        save();
    }
}

/// 清除标记（用户重新点击该曲时调用，允许重新尝试）。
pub(crate) fn unmark(id: i64) {
    let removed = {
        let mut g = IDS.lock().unwrap();
        let before = g.len();
        g.retain(|x| *x != id);
        before != g.len()
    };
    if removed {
        save();
    }
}

/// 清空全部标记。
pub(crate) fn clear_all() {
    IDS.lock().unwrap().clear();
    save();
}
