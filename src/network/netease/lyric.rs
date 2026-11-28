use serde_json::json;

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 获取歌词（lrc 原文 + tlyric 翻译）
    pub async fn song_lyric(&self, id: i64) -> Result<serde_json::Value, String> {
        let body = json!({ "id": id, "cp": false, "lv": 0, "kv": 0, "tv": -1 });
        self.post_weapi("/weapi/song/lyric", body).await
    }
}
