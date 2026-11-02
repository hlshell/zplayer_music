//! 资源 id 桥接：Slint 的 `int` 是 i32，而网易云歌单等资源 id 已超过 32 位。
//! 列表构建时把真实 64 位 id 登记成 (kind, 短 i32)，详情 / 收藏回调再按 (kind, 短 i32) 还原。
//! 按资源类型分表，避免歌单/专辑/电台/MV 的短 id 相互碰撞。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 表键：(资源类型, 短 id)。资源类型：0=歌单 1=专辑 2=歌手 3=电台 4=MV
type Key = (i32, i32);

fn table() -> &'static Mutex<HashMap<Key, i64>> {
    static T: OnceLock<Mutex<HashMap<Key, i64>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 登记真实 id 并返回可放进 Slint `int` 的短 id（未溢出时不做登记）。
pub fn remember(kind: i32, full: i64) -> i32 {
    let short = full as i32;
    if full != short as i64 {
        table().lock().unwrap().insert((kind, short), full);
    }
    short
}

/// 还原真实 id：优先查登记表，未登记时按无符号 32 位还原（兼容旧路径）。
pub fn resolve(kind: i32, short: i32) -> i64 {
    if let Some(v) = table().lock().unwrap().get(&(kind, short)) {
        return *v;
    }
    short as u32 as i64
}
