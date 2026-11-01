//! 网易云用户相关 API（收藏、播放记录；均需登录态 cookie，uid 由前端传入）

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 我喜欢的歌曲 ID 列表（uid 为网易云账号 id）
    pub async fn like_list(&self, uid: u64) -> Result<serde_json::Value, String> {
        self.post_weapi(
            &format!("/weapi/song/like/get?uid={}", uid),
            json!({ "uid": uid, "limit": 1000 }),
        )
        .await
    }

    /// 最近播放记录：type 0=所有 1=最近一周
    pub async fn record(&self, uid: u64, rec_type: Option<i64>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/v1/play/record",
            json!({ "uid": uid, "type": rec_type.unwrap_or(1) }),
        )
        .await
    }

    /// 对歌曲执行喜欢/取消喜欢（id 为歌曲 id，like=true 表示喜欢）
    pub async fn like_song(&self, id: u64, like: bool) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/radio/like",
            json!({ "alg": "itembased", "trackId": id, "like": like, "time": 25 }),
        )
        .await
    }

    /// 按 UID 查询用户公开信息（UID 登录用，仅查看公开数据，无需登录态）
    pub async fn user_detail(&self, uid: u64) -> Result<serde_json::Value, String> {
        self.post_weapi(
            &format!("/weapi/v1/user/detail?uid={}", uid),
            json!({ "uid": uid }),
        )
        .await
    }
}
