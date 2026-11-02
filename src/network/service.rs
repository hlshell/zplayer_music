//! 后端服务层：对迁移自 src-tauri 的 netease/sources 纯 Rust 模块做薄封装。
//! 去掉 tauri::command / State，暴露普通 async 函数，供 Slint UI 调用。
//! 数据以 serde_json::Value 返回（与旧前端 invoke 返回一致），UI 侧再映射到 Slint model。

use crate::network::netease::client::NeteaseClient;
use serde_json::Value;

/// 通用搜索：type 1=单曲 10=专辑 1000=歌单 1004=MV 1009=电台
pub async fn search(
    client: &NeteaseClient,
    keyword: &str,
    search_type: i64,
    limit: i64,
    offset: i64,
) -> Result<Value, String> {
    client.search(keyword, search_type, limit, offset).await
}

/// 单曲搜索
pub async fn search_songs(
    client: &NeteaseClient,
    keyword: &str,
    limit: i64,
    offset: i64,
) -> Result<Value, String> {
    client.search_songs(keyword, limit, offset).await
}

/// 搜索建议（下拉联想）
pub async fn search_suggest(client: &NeteaseClient, keywords: &str, limit: u32) -> Result<Value, String> {
    client.search_suggest(keywords, Some(limit)).await
}

/// 热搜榜（取第一名作为搜索框动态占位提示）
pub async fn search_hot_detail(client: &NeteaseClient) -> Result<Value, String> {
    client.get_plain("/search/hot/detail").await
}

/// 歌曲播放地址
pub async fn song_url(client: &NeteaseClient, ids: &[i64], br: i64) -> Result<Value, String> {
    client.song_url(ids, br).await
}

/// 歌曲歌词
pub async fn song_lyric(client: &NeteaseClient, id: i64) -> Result<Value, String> {
    client.song_lyric(id).await
}

/// 歌曲详情
pub async fn song_detail(client: &NeteaseClient, ids: &[i64]) -> Result<Value, String> {
    let ids_u64: Vec<u64> = ids.iter().map(|&i| i as u64).collect();
    client.song_detail(ids_u64).await
}

/// 每日推荐歌单（personalized）
pub async fn personalized(client: &NeteaseClient) -> Result<Value, String> {
    client.personalized(Some(30)).await
}

/// 每日推荐歌曲（data.dailySongs，需登录；未登录/失败由调用方回落榜单）
pub async fn recommend_songs(client: &NeteaseClient) -> Result<Value, String> {
    client.recommend_songs().await
}

/// 按分类取歌单列表（对齐 old getListByCat：/top/playlist?cat=）
pub async fn top_playlists_by_cat(client: &NeteaseClient, cat: &str, limit: u32, offset: u32) -> Result<Value, String> {
    client.top_playlists_by_cat(cat, limit, offset).await
}

/// 歌单分类目录（对齐 old getListByCat 分类来源：/api/playlist/catalogue）
pub async fn playlist_catalogue(client: &NeteaseClient) -> Result<Value, String> {
    client.playlist_catalogue().await
}

/// 推荐歌单
pub async fn recommend_playlist(client: &NeteaseClient, limit: u32) -> Result<Value, String> {
    client.recommend_playlist(Some(limit)).await
}

/// 排行榜列表
pub async fn toplist(client: &NeteaseClient) -> Result<Value, String> {
    client.toplist().await
}

/// 排行榜详情
pub async fn toplist_detail(client: &NeteaseClient, id: u64) -> Result<Value, String> {
    client.toplist_detail(id, None).await
}

/// 歌单详情
pub async fn playlist_detail(client: &NeteaseClient, id: u64, limit: u32) -> Result<Value, String> {
    client.playlist_detail(id, Some(limit)).await
}

/// 专辑详情（专辑信息 + 歌曲列表）
pub async fn album_songs(client: &NeteaseClient, id: u64) -> Result<Value, String> {
    client.album_songs(id).await
}

/// 歌手热门歌曲（返回 data.songs、data.total）
pub async fn artist_songs(client: &NeteaseClient, id: u64, limit: u32) -> Result<Value, String> {
    client.artist_songs(id, Some(limit), Some(0), Some("hot")).await
}

/// 新歌速递
pub async fn new_songs(client: &NeteaseClient, limit: u32) -> Result<Value, String> {
    client.new_songs(Some(limit)).await
}

/// 热门歌手
pub async fn top_artists(client: &NeteaseClient, limit: u32, offset: u32) -> Result<Value, String> {
    client.top_artists(Some(limit), Some(offset)).await
}

/// 新碟上架
pub async fn new_albums(
    client: &NeteaseClient,
    area: &str,
    limit: u32,
    offset: u32,
) -> Result<Value, String> {
    client.new_albums(Some(area), Some(limit), Some(offset)).await
}

/// 多音源兜底：网易云无法播放时按咪咕→酷我→酷狗→QQ→… 取可播 URL
pub async fn resolve_fallback(title: &str, artist: &str, duration: Option<u64>) -> Result<Value, String> {
    crate::network::sources::resolve_fallback(title, artist, duration).await
}

/// 按源解析可播 URL
pub async fn resolve_source(source: &str, id: &str, name: &str, artist: &str) -> Result<Value, String> {
    match crate::network::sources::resolve_source(source, id, name, artist).await {
        Some(url) => Ok(serde_json::json!({ "url": url, "source": source })),
        None => Ok(serde_json::json!({ "url": null, "source": source })),
    }
}

/// 全源聚合搜索（网易云 + 各第三方源）
pub async fn search_all(client: &NeteaseClient, keyword: &str, offset: u32, limit: u32) -> Result<Value, String> {
    use crate::network::sources::SourceSong;
    let kw = keyword.to_string();
    let c = client.clone();
    let ne_task = tokio::spawn(async move { c.search(&kw, 1, limit.max(1) as i64, offset as i64).await });
    let kw2 = keyword.to_string();
    let other_task = tokio::spawn(async move { crate::network::sources::search_all(&kw2).await });

    let mut items: Vec<SourceSong> = Vec::new();
    if let Ok(Ok(res)) = ne_task.await {
        if let Some(songs) = res.pointer("/result/songs").and_then(|v| v.as_array()) {
            for s in songs {
                let id = s.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                if id == 0 { continue; }
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if name.is_empty() { continue; }
                let artist = s.get("ar")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(|x| x.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("").to_string();
                let album = s.get("al").and_then(|x| x.get("name")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cover = s.get("al").and_then(|x| x.get("picUrl")).and_then(|v| v.as_str()).unwrap_or("").to_string();
                let duration = s.get("dt").and_then(|v| v.as_u64()).map(|d| d / 1000);
                items.push(SourceSong {
                    source: "netease",
                    id: id.to_string(),
                    name,
                    artist,
                    album,
                    cover,
                    duration,
                });
            }
        }
    }
    if let Ok(list) = other_task.await {
        items.extend(list);
    }
    let groups = crate::network::sources::group_songs(items);
    let mixed = crate::network::sources::round_robin_mix(groups);
    Ok(serde_json::to_value(mixed).unwrap_or_default())
}

/// 个性化推荐节目（电台浏览唯一可用源；返回 result 列表，每条含 program.radio 电台元信息 + program.mainSong 音频）
pub async fn dj_personalized_recommend(client: &NeteaseClient, limit: u32) -> Result<Value, String> {
    client.dj_personalized_recommend(limit).await
}

/// 全部 MV（对齐 old /mv/all 公共 GET；data 字段与解析一致）
pub async fn mv_all(client: &NeteaseClient, area: &str, limit: u32, offset: u32) -> Result<Value, String> {
    client.mv_all(area, limit, offset).await
}

/// 精选 MV 列表（分页）
pub async fn mv_exclusive(client: &NeteaseClient, limit: u32, offset: u32) -> Result<Value, String> {
    client.mv_exclusive(limit, offset).await
}

/// MV 详情（含简介/歌手/封面，无歌曲列表）
pub async fn mv_detail(client: &NeteaseClient, id: u64) -> Result<Value, String> {
    client.mv_detail(id).await
}
