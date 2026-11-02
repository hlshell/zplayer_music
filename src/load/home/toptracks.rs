//! 榜单详情：拉取榜单/歌单 tracks（含多榜单兜底逻辑由 mod.rs 调度，此处单榜拉取）。

use serde_json::Value;
use crate::network::netease::client::NeteaseClient;
use crate::network::service;

/// 拉取榜单/歌单详情：返回 (tracks, 歌单封面 URL)。
/// weapi playlist/detail 有两种返回包裹：旧版 /playlist/*、v3 /result/*，兼容两种取先非空者。
pub async fn tl_det_tracks(client: &NeteaseClient, id: u64) -> (Vec<Value>, String) {
    match service::toplist_detail(client, id).await {
        Ok(v) => {
            let cover = v.pointer("/playlist/coverImgUrl")
                .or_else(|| v.pointer("/result/coverImgUrl"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let tracks = v.pointer("/playlist/tracks")
                .filter(|t| t.as_array().map(|a| !a.is_empty()).unwrap_or(false))
                .or_else(|| v.pointer("/result/tracks"))
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            if tracks.is_empty() {
                let top_keys: Vec<String> = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
                let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
                log::info!("[home] 榜单{} 无 tracks: code={} top_keys={:?}", id, code, top_keys);
            }
            (tracks, cover)
        }
        Err(e) => { log::info!("[home] 榜单{} 请求失败: {}", id, e); (Vec::new(), String::new()) }
    }
}
