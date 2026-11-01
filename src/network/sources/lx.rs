//! 洛雪（LX Music）音源脚本执行器 —— 对齐 old 前端的 LxMusicSourceRunner：
//! 用内嵌 QuickJS 沙箱执行用户导入的音源脚本（.js），并把脚本用到的
//! httpRequest/httpFetch 桥接到 Rust 的 reqwest（阻塞式，QuickJS 单线程）。
//!
//! 脚本规范（新版 musicSources）：
//!   const musicSources = { 源key: { getSearchResults(kw) => [...], getMusicUrl(info, quality) => url } }
//! 这里把脚本 + 一段桥 JS 拼成一段 eval 执行，Rust 只调用全局桥函数，
//! 桥函数内部引用脚本的 musicSources 词法变量。
//!
//! 网络桥：__lxHttp（Rust 注入）返回 JSON 字符串，桥 JS 里 JSON.parse 成对象，
//! 避免 Rust 闭包返回"绑定 JS 生命周期"的对象的 lifetime 问题。

use rquickjs::{Context, Function, Runtime, Value};

use std::sync::OnceLock;

fn blocking_client() -> &'static reqwest::blocking::Client {
    static C: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .cookie_store(true)
            .timeout(std::time::Duration::from_secs(15))
            .connect_timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new())
    })
}

/// 桥 JS：挂在全局的调用入口 + 对 httpRequest/httpFetch 的兼容包装。
const BRIDGE_JS: &str = r#"
(function () {
  var __parse = function (raw) {
    return typeof raw === 'string' ? JSON.parse(raw) : raw;
  };
  if (typeof globalThis.httpRequest !== 'function') {
    globalThis.httpRequest = function (url, options) {
      return __parse(globalThis.__lxHttp(url, options || {}));
    };
  }
  if (typeof globalThis.httpFetch !== 'function') {
    globalThis.httpFetch = async function (url, options) {
      var r = __parse(globalThis.__lxHttp(url, options || {}));
      return {
        ok: r.statusCode >= 200 && r.statusCode < 400,
        status: r.statusCode,
        headers: r.headers,
        json: async function () { return JSON.parse(r.body); },
        text: async function () { return r.body; }
      };
    };
  }
  globalThis.__lxGetMusicUrl = function (source, infoJson, quality) {
    var info = JSON.parse(infoJson);
    return musicSources[source].getMusicUrl(info, quality);
  };
  globalThis.__lxGetSources = function () {
    return Object.keys(musicSources);
  };
})();
"#;

/// 一次性的音源脚本执行器（MVP：每次调用新建 QuickJS 上下文，跑通后可按 old
/// Worker 模型升级为常驻线程）。
pub struct LxSourceRunner {
    /// 拼接后的完整脚本（原脚本 + 桥）
    full_script: String,
    /// 支持的音源 key 列表
    sources: Vec<String>,
}

impl LxSourceRunner {
    /// 用脚本原文构造执行器；立即在新上下文里初始化并读出支持的音源。
    pub fn new(script: &str) -> Result<Self, String> {
        let full_script = format!("{}\n{}", script, BRIDGE_JS);
        let sources: Vec<String> = run_ctx(&full_script, |ctx| {
            ctx.eval::<Vec<String>, _>("__lxGetSources()").ok()
        })
        .ok_or_else(|| "音源脚本初始化失败（musicSources 不存在或语法错误）".to_string())?;
        Ok(Self { full_script, sources })
    }

    pub fn sources(&self) -> &[String] {
        &self.sources
    }

    /// 用脚本 getMusicUrl 解析某音源下歌曲的可播 URL。
    /// music_info 会序列化为 JSON 传给脚本（对齐 old convertToLxMusicInfo 的字段）。
    pub fn get_music_url(&self, source: &str, music_info: &serde_json::Value, quality: &str) -> Option<String> {
        let info_json = serde_json::to_string(music_info).ok()?;
        let code = format!(
            "__lxGetMusicUrl({}, {}, {})",
            js_str(source),
            js_str(&info_json),
            js_str(quality)
        );
        run_ctx(&self.full_script, |ctx| {
            let v: Value = ctx.eval(code.as_str()).ok()?;
            // getMusicUrl 是 async → Promise；finish 驱动 job queue 直到 resolve
            let p = v.as_promise()?;
            let res: Value = p.finish().ok()?;
            res.as_string().and_then(|s| s.to_string().ok())
        })
    }
}

/// 在新 Runtime+Context 里执行闭包（每次独立上下文；rquickjs 非 Send，全在同一线程完成）。
fn run_ctx<T>(full_script: &str, f: impl for<'a> FnOnce(rquickjs::Ctx<'a>) -> Option<T>) -> Option<T> {
    let rt = Runtime::new().ok()?;
    let ctx = Context::full(&rt).ok()?;
    ctx.with(|c| {
        // 注入阻塞 HTTP 桥（返回 JSON 字符串，桥 JS 负责 parse）
        let http_fn = Function::new(
            c.clone(),
            |url: String, options: Value| -> rquickjs::Result<String> { http_request(&url, &options) },
        )
        .ok()?;
        c.globals().set("__lxHttp", http_fn).ok()?;
        // 执行脚本 + 桥
        c.eval::<Value, _>(full_script).ok()?;
        f(c)
    })
}

/// Rust 侧真实阻塞 HTTP：返回 {statusCode, headers, body} 的 JSON 文本。
fn http_request<'js>(url: &str, options: &Value<'js>) -> rquickjs::Result<String> {
    let mut method = "GET".to_string();
    let mut headers = std::collections::HashMap::<String, String>::new();
    let mut body: Option<String> = None;
    if let Some(obj) = options.as_object() {
        if let Ok(mv) = obj.get::<_, Value>("method") {
            if let Some(s) = mv.as_string().and_then(|s| s.to_string().ok()) {
                method = s;
            }
        }
        if let Ok(hv) = obj.get::<_, Value>("headers") {
            if let Some(hobj) = hv.as_object() {
                for k in hobj.keys::<String>() {
                    let Ok(k) = k else { continue };
                    if let Ok(v) = hobj.get::<_, Value>(k.clone()) {
                        if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
                            headers.insert(k.to_string(), s);
                        }
                    }
                }
            }
        }
        if let Ok(bv) = obj.get::<_, Value>("body") {
            if let Some(s) = bv.as_string().and_then(|s| s.to_string().ok()) {
                body = Some(s);
            }
        }
    }
    headers.entry("User-Agent".into()).or_insert_with(|| {
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36".to_string()
    });

    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = blocking_client().request(m, url);
    for (k, v) in &headers {
        if let Ok(hn) = reqwest::header::HeaderName::from_bytes(k.as_bytes()) {
            req = req.header(hn, v);
        }
    }
    if let Some(b) = &body {
        if method == "POST" || method == "PUT" {
            req = req.body(b.clone());
        }
    }

    let (status, resp_headers, resp_body) = match req.send() {
        Ok(r) => {
            let st = r.status().as_u16();
            let hs: std::collections::HashMap<String, String> = r
                .headers()
                .iter()
                .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
                .collect();
            let txt = r.text().unwrap_or_default();
            (st, hs, txt)
        }
        Err(_) => (0u16, std::collections::HashMap::new(), String::new()),
    };
    let obj = serde_json::json!({ "statusCode": status, "headers": resp_headers, "body": resp_body });
    Ok(serde_json::to_string(&obj).map_err(|_| rquickjs::Error::Unknown)?)
}

fn js_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
    format!("\"{}\"", escaped)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn lx_simple_script() {
        // 简化洛雪脚本：getMusicUrl 不依赖网络直接返回 URL
        let script = r#"
const musicSources = {
  demo: {
    getSearchResults: async (kw) => [{ id: '1', name: kw, artist: '周杰伦' }],
    getMusicUrl: async (info, quality) => 'http://127.0.0.1/demo-' + info.songmid + '.mp3',
  }
};
"#;
        let runner = super::LxSourceRunner::new(script).expect("init");
        assert_eq!(runner.sources(), &["demo"]);
        let info = json!({ "songmid": "abc", "name": "晴天", "singer": "周杰伦" });
        let url = runner.get_music_url("demo", &info, "320k").expect("url");
        assert_eq!(url, "http://127.0.0.1/demo-abc.mp3");
    }
}
