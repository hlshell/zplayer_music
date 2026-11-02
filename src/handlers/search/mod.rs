//! 搜索页/歌单页/专辑页/搜索建议回调装配。
//!
//! 按职责拆分为子模块：
//! - [`playlists`]：歌单分类切换 + 触底分页
//! - [`albums`]：专辑分类切换 + 触底分页
//! - [`songs`]：搜索（关键词搜索 + 播放全部）
//! - [`suggest`]：搜索建议（防抖 + 序号丢弃过期响应）

use super::Ctx;

mod albums;
mod playlists;
mod songs;
mod suggest;
mod typed;

pub fn setup(ctx: &Ctx) {
    playlists::setup_playlists(ctx);
    albums::setup_albums(ctx);
    songs::setup_songs(ctx);
    suggest::setup_suggest(ctx);
}
