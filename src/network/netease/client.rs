//! 网易云 API HTTP 客户端：reqwest + cookie jar + weapi 请求封装

use reqwest::header::{HeaderMap, COOKIE, REFERER, USER_AGENT};
use reqwest::Client;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::network::netease::crypto::weapi_encrypt;

const BASE_URL: &str = "https://music.163.com";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36";

/// 构建底层 reqwest client（默认头 + 全局代理/realip 配置）
fn build_http_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, UA.parse().unwrap());
    headers.insert(REFERER, "https://music.163.com/".parse().unwrap());
    crate::network::proxy::apply(
        Client::builder()
            .default_headers(headers)
            .cookie_store(true)
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(8)),
    )
    .build()
    .unwrap_or_else(|_| Client::new())
}

#[derive(Clone)]
pub struct NeteaseClient {
    inner: Arc<Inner>,
}

struct Inner {
    client: std::sync::Mutex<Client>,
    client_version: std::sync::atomic::AtomicU64,
    cookie: Mutex<String>,
}

impl NeteaseClient {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                client: std::sync::Mutex::new(build_http_client()),
                client_version: std::sync::atomic::AtomicU64::new(crate::network::proxy::current_version()),
                cookie: Mutex::new(String::new()),
            }),
        }
    }

    /// 若全局网络配置版本变化则重建底层 reqwest client（代理/realip 真实生效）
    fn ensure_client(&self) -> Client {
        let v = crate::network::proxy::current_version();
        let inner = &self.inner;
        let cached = inner.client_version.load(std::sync::atomic::Ordering::Relaxed);
        if v != cached {
            let mut lock = inner.client.lock().unwrap();
            *lock = build_http_client();
            inner.client_version.store(v, std::sync::atomic::Ordering::Relaxed);
        }
        inner.client.lock().unwrap().clone()
    }

    /// 读取当前 cookie（登录态持久化用）
    pub async fn get_cookie(&self) -> String {
        self.inner.cookie.lock().await.clone()
    }

    /// 注入 cookie（启动时从本地恢复登录态）
    pub async fn set_cookie(&self, cookie: String) {
        let mut cur = self.inner.cookie.lock().await;
        *cur = cookie;
    }

    /// 清空 cookie（登出）
    pub async fn clear_cookie(&self) {
        let mut cur = self.inner.cookie.lock().await;
        *cur = String::new();
    }

    /// 发起 weapi POST 请求
    pub async fn post_weapi(&self, path: &str, body: Value) -> Result<Value, String> {
        let json_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
        let (params, enc_sec_key) = weapi_encrypt(&json_text);

        let form = [("params", params), ("encSecKey", enc_sec_key)];
        let url = format!("{}{}", BASE_URL, path);

        log::info!("weapi POST {} body={}", url, json_text);

        // 传输层失败（连接重置/超时/临时 DNS 抖动）重试一次：
        // weapi 请求在本应用内均为只读查询，重试安全，可显著减少偶发“加载失败”。
        let mut resp = None;
        for attempt in 0..2 {
            let mut req = self.ensure_client().post(&url).form(&form);
            let cookie = self.inner.cookie.lock().await.clone();
            if !cookie.is_empty() {
                req = req.header(COOKIE, cookie);
            }
            match req.send().await {
                Ok(r) => {
                    resp = Some(r);
                    break;
                }
                Err(e) => {
                    if attempt == 1 {
                        return Err(format!("request failed: {}", e));
                    }
                    log::warn!("weapi POST {} 发送失败，重试一次: {}", url, e);
                }
            }
        }
        let resp = resp.expect("weapi 请求至少尝试一次");
        let status = resp.status();

        let set_cookies: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(|s| s.split(';').next().unwrap_or("").to_string())
            .filter(|s| !s.is_empty() && s.contains('='))
            .collect();
        if !set_cookies.is_empty() {
            let mut cur = self.inner.cookie.lock().await;
            *cur = merge_cookies(&cur, &set_cookies);
        }

        let text = resp.text().await.map_err(|e| e.to_string())?;
        log::info!("weapi {} -> {} ({} bytes)", url, status, text.len());

        if !status.is_success() {
            return Err(format!(
                "HTTP {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }
        serde_json::from_str::<Value>(&text).map_err(move |e| {
            format!(
                "json parse failed: {}; body={}",
                e,
                text.chars().take(500).collect::<String>()
            )
        })
    }

    /// 普通 GET 请求（无需 weapi 加密，如 /search/hot/detail），对齐 old
    pub async fn get_plain(&self, path: &str) -> Result<Value, String> {
        self.get_plain_query(path, &[]).await
    }

    /// 普通 GET 请求（带 query 参数，如 /api/mv/all?limit=&offset=），对齐 old 的 REST 接口
    pub async fn get_plain_query(&self, path: &str, params: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{}{}", BASE_URL, path);
        let mut req = self.ensure_client().get(&url).query(params);
        let cookie = self.inner.cookie.lock().await.clone();
        if !cookie.is_empty() {
            req = req.header(COOKIE, cookie);
        }
        let resp = req.send().await.map_err(|e| format!("request failed: {}", e))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        log::info!("GET {} -> {} ({} bytes)", url, status, text.len());
        if !status.is_success() {
            return Err(format!(
                "HTTP {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }
        serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("json parse failed: {}; body={}", e, text.chars().take(500).collect::<String>()))
    }

    /// 搜索（对齐 old /cloudsearch 的公开接口形式：GET /api/search/get）。
    /// weapi /weapi/cloudsearch/get/web 当前被网易云风控（code 50000005），
    /// 公开 GET 接口可正常返回。type 1=单曲 10=专辑 1000=歌单 1004=MV 1009=电台。
    pub async fn search_get(&self, keyword: &str, search_type: i64, limit: i64, offset: i64) -> Result<Value, String> {
        let url = format!("{}/api/search/get", BASE_URL);
        let mut req = self.ensure_client().get(&url).query(&[
            ("s", keyword),
            ("type", &search_type.to_string()),
            ("limit", &limit.to_string()),
            ("offset", &offset.to_string()),
        ]);
        let cookie = self.inner.cookie.lock().await.clone();
        if !cookie.is_empty() {
            req = req.header(COOKIE, cookie);
        }
        let resp = req.send().await.map_err(|e| format!("search request failed: {}", e))?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        log::info!("GET {} -> {} ({} bytes)", url, status, text.len());
        if !status.is_success() {
            return Err(format!(
                "HTTP {}: {}",
                status,
                text.chars().take(500).collect::<String>()
            ));
        }
        serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("json parse failed: {}; body={}", e, text.chars().take(500).collect::<String>()))
    }

    /// 获取歌单分类目录（官方 weapi：/weapi/playlist/catalogue）
    /// 响应结构：{ code, sub:[{name, category,...}], categories:{...}, category_groups:[...] }
    pub async fn playlist_catalogue(&self) -> Result<Value, String> {
        // 用更通用的 /weapi/playlist/catlist（返回 all/categories/sub，sub 为子分类含 name）
        self.post_weapi("/weapi/playlist/catlist", serde_json::json!({})).await
    }

    /// 按分类取歌单列表（官方 weapi：/weapi/playlist/list，对齐 old getListByCat 的 cat/offset/limit）
    /// 响应结构：{ playlists: [...] }，每条含 id/name/playCount/coverImgUrl。
    pub async fn top_playlists_by_cat(&self, cat: &str, limit: u32, offset: u32) -> Result<Value, String> {
        self.post_weapi(
            "/weapi/playlist/list",
            serde_json::json!({
                "cat": cat,
                "order": "hot",
                "limit": limit,
                "offset": offset,
                "total": true,
            }),
        )
        .await
    }

    /// 全部 MV（对齐 old getTopMv/getAllMv 的 /mv/all，公共 GET 可直接取到真实数据）。
    /// 响应结构：{ data: [{ id, name, artistName, cover, playCount, duration }], count }。
    /// 注：网抑云当前服务端忽略 area 过滤（各地区返回同一列表），故 area 参数保留但浏览侧固定传空。
    pub async fn mv_all(&self, area: &str, limit: u32, offset: u32) -> Result<Value, String> {
        self.get_plain_query(
            "/api/mv/all",
            &[
                ("area", area.to_string()),
                ("limit", limit.to_string()),
                ("offset", offset.to_string()),
                ("order", "false".to_string()),
            ],
        )
        .await
    }
}

fn merge_cookies(old: &str, news: &[String]) -> String {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for pair in old.split(';').filter(|s| !s.trim().is_empty()) {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    for pair in news {
        if let Some((k, v)) = pair.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map.into_iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join("; ")
}

impl Default for NeteaseClient {
    fn default() -> Self {
        Self::new()
    }
}
