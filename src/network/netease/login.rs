//! 网易云二维码登录与账号

use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

use super::client::NeteaseClient;

impl NeteaseClient {
    /// 获取二维码 key（unikey）
    pub async fn qr_key(&self) -> Result<serde_json::Value, String> {
        self.post_weapi(
            "/weapi/login/qrcode/unikey?timestamp={}&noCookie=true",
            json!({ "type": 1 }),
        )
        .await
    }

    /// 轮询扫码状态：code 800=过期 801=等待扫码 802=已扫码待确认 803=登录成功（成功时已写入 cookie）
    pub async fn qr_check(&self, unikey: &str) -> Result<serde_json::Value, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        self.post_weapi(
            "/weapi/login/qrcode/client/login",
            json!({ "unikey": unikey, "timestamp": ts }),
        )
        .await
    }

    /// 当前登录用户信息（需登录态）
    pub async fn account(&self) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/nuser/account/get", json!({})).await
    }

    /// 登出
    pub async fn logout(&self) -> Result<serde_json::Value, String> {
        self.post_weapi("/weapi/logout", json!({})).await
    }
}
