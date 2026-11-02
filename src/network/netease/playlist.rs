//! 网易云歌单 API

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 歌单详情（含 tracks；详情页歌单卡片使用）
    /// 注意：旧接口 /weapi/playlist/detail 对普通歌单 + 大 s 会拒绝（返回 code/msg），
    /// 标准做法是 v6 接口 + s=8（收藏者采样数惯例值），n 控制返回曲目数
    pub async fn playlist_detail(&self, id: u64, limit: Option<u32>) -> Result<serde_json::Value, String> {
        let n = limit.unwrap_or(1000);
        self.post_weapi(
            "/weapi/v6/playlist/detail",
            json!({ "id": id, "n": n, "s": 8 }),
        )
        .await
    }

    /// 用户的歌单列表（需登录态，uid 可通过 cookie 获取）
    pub async fn user_playlist(&self, uid: u64, limit: Option<u32>, offset: Option<u32>) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/user/playlist",
            json!({ "uid": uid, "limit": limit.unwrap_or(30), "offset": offset.unwrap_or(0), "total": true }),
        )
        .await
    }
}
