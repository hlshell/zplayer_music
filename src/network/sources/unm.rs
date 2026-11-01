//! UnblockNeteaseMusic（unm_engine Rust 版）兜底音源。
//! 用官方 `unm_api_utils::build_full_executor()` 注册 kugou/pyncm/kuwo/qq 等
//! 引擎，并手动补注册 migu；只对音乐播放相关引擎做 search + retrieve，
//! 返回原曲相符的可播 URL。这是对 old 里 `@unblockneteasemusic/server`
//! （unblock 解锁策略）的 Rust 等价迁移。

use std::borrow::Cow;
use std::sync::Arc;

use super::{MultiSourceClient, ResolvedSong};
use unm_types::{Artist, Context, Song};

pub const NAME: &str = "unm";

/// 用于音乐播放兜底的引擎（migu/kugou/kuwo/pyncm 为 old unblock 平台，qq 增强）。
/// 跳过 build_full_executor 里的 bilibili（视频）/ ytdl（需外部 exe）/ joox（需 cookie）。
const SEARCH_ENGINES: &[&str] = &["migu", "kugou", "kuwo", "pyncm", "qq"];

fn build_executor() -> unm_engine::executor::Executor {
    let mut executor = unm_api_utils::executor::build_full_executor();
    // build_full_executor 未含 migu，手动补注册
    executor.register(Cow::Borrowed(unm_engine_migu::ENGINE_ID), Arc::new(unm_engine_migu::MiguEngine));
    executor
}

/// 拆分歌手（`、` / `&` / `，` 分隔）为数组，供 UNM 匹配。
fn split_artists(s: &str) -> Vec<String> {
    s.split(['、', '&', '＆', ',', '，'])
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| x.to_string())
        .collect()
}

/// 用 UNM 解析可播 URL：search（多引擎并行/顺序找匹配）→ retrieve 取直链。
pub async fn try_resolve(_c: &MultiSourceClient, name: &str, artist: &str) -> Option<ResolvedSong> {
    if name.trim().is_empty() {
        return None;
    }
    let executor = build_executor();
    let song = Song::builder()
        .name(name.to_string())
        .artists(
            split_artists(artist)
                .into_iter()
                .map(|n| Artist::builder().name(n).build())
                .collect(),
        )
        .build();
    let ctx = Context::default();
    let ids: Vec<Cow<'static, str>> = SEARCH_ENGINES
        .iter()
        .map(|s| Cow::Borrowed(*s))
        .collect();
    let search = executor.search(&ids, &song, &ctx).await.ok()?;
    let rel = executor.retrieve(&search, &ctx).await.ok()?;
    if rel.url.is_empty() {
        return None;
    }
    Some(ResolvedSong {
        url: rel.url,
        source: NAME,
        name: name.to_string(),
        artist: artist.to_string(),
        platform_id: None,
        duration: None,
    })
}
