//! 网易云专辑 API

use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 专辑详情（含专辑信息 + 歌曲列表）
    pub async fn album_detail(&self, id: u64) -> Result<serde_json::Value, String> {
        self.post_weapi(&format!("/weapi/v1/album/{}", id), json!({})).await
    }

    /// 专辑内全部歌曲
    pub async fn album_songs(&self, id: u64) -> Result<serde_json::Value, String> {
        self.post_weapi(&format!("/weapi/v1/album/{}", id), json!({})).await
    }
}
