//! 网易云排行榜 API

use serde_json::json;

use crate::network::netease::client::NeteaseClient;

impl NeteaseClient {
    /// 获取全部排行榜列表
    pub async fn toplist(&self) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/toplist", json!({})).await
    }

    /// 获取排行榜详情（歌曲列表）
    pub async fn toplist_detail(&self, id: u64, limit: Option<u32>) -> Result<serde_json::Value, String> {
        let n = limit.unwrap_or(100);
        self.post_weapi(
            "/weapi/playlist/detail",
            // 缺 s 时新版接口 tracks 为空（曾导致排行榜"暂无歌曲"）
            json!({ "id": id, "n": n, "s": n }),
        )
        .await
    }
}
