//! 网易云补充常用接口：banner / 搜索建议 / 相似歌曲 / 每日推荐歌单 / 评论 / 收藏歌单

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 发现页 Banner
    pub async fn banner(&self, _type: Option<i64>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/v2/banner/get",
            json!({ "clientType": _type.unwrap_or(0), "type": 0 }),
        )
        .await
    }

    /// 搜索建议（下拉联想）
    pub async fn search_suggest(&self, keywords: &str, limit: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/search/suggest/web",
            json!({ "s": keywords, "limit": limit.unwrap_or(8) }),
        )
        .await
    }

    /// 相似歌曲
    pub async fn simi_song(&self, id: u64, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/v1/discovery/simiSong",
            json!({ "songid": id, "limit": limit.unwrap_or(10), "offset": offset.unwrap_or(0) }),
        )
        .await
    }

    /// 相关歌单（歌曲详情页的相关推荐）
    pub async fn related_playlist(&self, id: u64) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/related/playlist",
            json!({ "id": id }),
        )
        .await
    }

    /// 每日推荐歌单（需登录）
    pub async fn recommend_playlist(&self, limit: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/personalized/playlist",
            json!({ "limit": limit.unwrap_or(10), "n": limit.unwrap_or(10), "total": true }),
        )
        .await
    }

    /// 歌曲热门评论
    pub async fn song_comment(&self, id: u64, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            &format!("/weapi/v1/resource/comments/R_SO_4_{}", id),
            json!({ "rid": format!("R_SO_4_{}", id), "limit": limit.unwrap_or(20), "offset": offset.unwrap_or(0) }),
        )
        .await
    }

    /// 收藏/取消收藏歌单（add=true 收藏，false 取消；需登录）
    pub async fn playlist_subscribe(&self, id: u64, add: bool) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/playlist/subscribe",
            json!({ "id": id, "add": add }),
        )
        .await
    }

    /// 创建歌单（需登录）
    pub async fn playlist_create(&self, name: &str) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/playlist/create",
            json!({ "name": name, "privacy": 0 }),
        )
        .await
    }
}
