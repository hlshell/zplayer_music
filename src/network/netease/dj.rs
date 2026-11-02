//! 网易云电台 API

use serde_json::{json, Value};

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 个性化推荐节目（唯一仍可用的电台浏览源）：
    /// 网抑云已停用 /weapi/dj/catelist、/dj/recommend/type、/dj/radio/hot、/dj/program 等路由
    /// （均返回 404 接口未找到），仅 /weapi/personalized/djprogram 正常返回。
    /// 响应结构：{ result: [ { id, name, picUrl, copywriter, program: { radio: {id,name,picUrl,desc,subCount}, mainSong: {...} } } ] }
    pub async fn dj_personalized_recommend(&self, limit: u32) -> Result<Value, String> {
        self.post_weapi(
            "/weapi/personalized/djprogram",
            json!({ "limit": limit }),
        )
        .await
    }
}
