//! 网易云歌手 API

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 歌手详情（基础信息 + 热门 50 首）
    pub async fn artist_detail(&self, id: u64) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/artist/detail", json!({ "id": id })).await
    }

    /// 歌手歌曲（order: hot/热门, time/最新）；返回 data.songs、data.total
    pub async fn artist_songs(
        &self,
        id: u64,
        limit: Option<u32>,
        offset: Option<u32>,
        order: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/v1/artist/songs",
            json!({
                "id": id,
                "limit": limit.unwrap_or(50),
                "offset": offset.unwrap_or(0),
                "order": order.unwrap_or("hot"),
                "total": true,
            }),
        )
        .await
    }

    /// 歌手专辑
    pub async fn artist_album(&self, id: u64, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            &format!("/weapi/artist/albums/{}", id),
            json!({ "limit": limit.unwrap_or(30), "offset": offset.unwrap_or(0), "total": true }),
        )
        .await
    }
}
