//! `load` 模块入口：首页数据加载。
//! 主入口 `load_initial`（并行拉取发现页各板块并填充 Slint 模型）+ 封面管道 / 各板块 builder，
//! 均位于 `home/` 子模块；本入口对上层 re-export，保持 `crate::load::X` 调用路径不变。

pub mod home;

// re-export 保持 crate::load::X 调用路径；部分符号上层未直接使用（保留 API 兼容）。
#[allow(unused_imports)]
pub use home::{
    cover_now, cover_warm, fetch_covers, from, load_initial, model, parse_tracks, set_runtime,
    to, warm_rows, SongBase,
};
#[allow(unused_imports)]
pub use home::{HERO_COVER_URL, HERO_POOL};
