use super::client::NeteaseClient;

impl NeteaseClient {
    /// 通用搜索：type 1=单曲 10=专辑 1000=歌单 1004=MV 1009=电台
    /// 走公开 GET /api/search/get（对齐 old /cloudsearch）——weapi cloudsearch 当前被风控（50000005）。
    pub async fn search(
        &self,
        keyword: &str,
        search_type: i64,
        limit: i64,
        offset: i64,
    ) -> Result<serde_json::Value, String> {
        self.search_get(keyword, search_type, limit, offset).await
    }

    pub async fn search_songs(&self, keyword: &str, limit: i64, offset: i64) -> Result<serde_json::Value, String> {
        self.search(keyword, 1, limit, offset).await
    }
}
