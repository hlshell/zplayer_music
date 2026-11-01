use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    pub async fn song_url(&self, ids: &[i64], br: i64) -> Result<serde_json::Value, String> {
        let body = json!({
            "ids": ids,
            "br": br,
            "offset": 0,
            "total": true,
            "limit": 1
        });
        let v = self.post_weapi("/weapi/song/enhance/player/url", body).await?;
        // 网易云返回 { data: [ { id, url, ... } ] }
        Ok(v.get("data").cloned().unwrap_or(serde_json::Value::Null))
    }
}
