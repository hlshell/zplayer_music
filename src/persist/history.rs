//! 搜索历史：本地 search_history.json 持久化 + 内存集合。
//! 记录搜索过的关键词（去重按词，最新在前），支持单条删除与清空。

use std::sync::Mutex;

/// 上限保留 20 条。
const MAX: usize = 20;

static HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(crate) fn load_history() {
    let Some(s) = crate::platform::storage::read_text("search_history.json") else { return; };
    if let Ok(v) = serde_json::from_str::<Vec<String>>(&s) {
        // 去重（保留首个 = 最新），清理历史版本可能残留的重复条目
        let mut seen = std::collections::HashSet::new();
        let v: Vec<String> = v.into_iter().filter(|w| !w.is_empty() && seen.insert(w.clone())).collect();
        *HISTORY.lock().unwrap() = v;
    }
}

fn save_history() {
    if let Ok(s) = serde_json::to_string(&*HISTORY.lock().unwrap()) {
        let _ = crate::platform::storage::write_text("search_history.json", &s);
    }
}

/// 记录一次搜索（词去重并前置）。
pub(crate) fn record(word: &str) {
    let word = word.trim();
    if word.is_empty() { return; }
    let mut g = HISTORY.lock().unwrap();
    if let Some(pos) = g.iter().position(|w| w == word) {
        g.remove(pos);
    }
    g.insert(0, word.to_string());
    g.truncate(MAX);
    drop(g);
    save_history();
}

pub(crate) fn list() -> Vec<String> {
    HISTORY.lock().unwrap().clone()
}

pub(crate) fn remove(word: &str) {
    let mut g = HISTORY.lock().unwrap();
    if let Some(pos) = g.iter().position(|w| w == word) {
        g.remove(pos);
        drop(g);
        save_history();
    }
}

pub(crate) fn clear() {
    HISTORY.lock().unwrap().clear();
    save_history();
}
