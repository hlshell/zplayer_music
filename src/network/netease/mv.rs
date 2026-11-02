//! 网易云 MV API

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// MV 详情（含各清晰度地址 brs）
    pub async fn mv_detail(&self, id: u64) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/mv/detail", json!({ "id": id, "type": "mp4" })).await
    }

    /// MV 播放地址（推荐清晰度）
    pub async fn mv_url(&self, id: u64, r: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/song/enhance/play/mv/url",
            json!({ "id": id, "r": r.unwrap_or(1080), "br": 0 }),
        )
        .await
    }

    /// 精选 MV 列表（分页）：{ data: [{ id, name, artistName, cover, playCount, duration }] }
    pub async fn mv_exclusive(&self, limit: u32, offset: u32) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/mv/exclusive/rcmd",
            json!({ "limit": limit, "offset": offset }),
        )
        .await
    }
}
