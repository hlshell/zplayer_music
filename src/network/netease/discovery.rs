//! 网易云热搜 & 推荐歌单 API

use serde_json::json;

use crate::network::netease::client::NeteaseClient;

impl NeteaseClient {
    /// 热搜列表（对齐 old：GET /search/hot/detail）
    pub async fn search_hot(&self) -> Result<serde_json::Value, String> {
        self.get_plain("/search/hot/detail").await
    }

    /// 推荐歌单
    pub async fn personalized(&self, limit: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/personalized/playlist",
            json!({ "limit": limit.unwrap_or(10), "total": true }),
        )
        .await
    }

    /// 新歌速递（对齐 old：/personalized/newsong 推荐新歌）
    pub async fn new_songs(&self, limit: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/personalized/newsong",
            json!({ "areaId": 0, "total": true, "limit": limit.unwrap_or(30) }),
        )
        .await
    }

    /// 热门歌手
    pub async fn top_artists(&self, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/artist/top",
            json!({ "limit": limit.unwrap_or(20), "offset": offset.unwrap_or(0), "total": true }),
        )
        .await
    }

    /// 新碟上架
    pub async fn new_albums(&self, area: Option<&str>, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/album/new",
            json!({
                "area": area.unwrap_or("ALL"),
                "limit": limit.unwrap_or(20),
                "offset": offset.unwrap_or(0),
                "type": "new",
                "total": true,
            }),
        )
        .await
    }

    /// 每日推荐歌曲（对齐 old：GET /recommend/songs → 网易 v3/discovery/recommend/songs，固定 30 首）
    /// 需登录态；未登录返回 code=301，由调用方回落榜单数据
    pub async fn recommend_songs(&self) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/v3/discovery/recommend/songs", json!({})).await
    }

    /// 歌曲详情
    pub async fn song_detail(&self, ids: Vec<u64>) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/song/detail", json!({ "ids": ids })).await
    }
}
