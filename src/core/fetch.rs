//! 音频字节获取（播放/预取共用）+ 网络响应解析工具。
//! 正式播放与后台预取共用 fetch_song_bytes，使点上一首/下一首时目标歌曲
//! 大概率已在缓存中，从而近乎立即出声。

use crate::network::netease::client::NeteaseClient;

/// 歌单分页加载状态（滚动触底追加下一页）。
/// items 只存 Send 字段（封面在 UI 线程由这些字段构造），保证跨线程安全。
pub(crate) struct Paging {
    pub cat: i32,
    pub offset: u32,
    /// 分类切换代数：切分类时 +1，旧分类的迟到响应据此丢弃（避免串页/错位）
    pub epoch: u64,
    pub end: bool,
    pub loading: bool,
    pub items: Vec<(i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color)>,
}

/// 下载专用 Client：全局复用（切歌/预取不再各自重建连接池与 TLS 握手），
/// 按代理配置版本号自动重建，使设置页改代理后即时生效；带读超时防读挂死。
///
/// 注意：wasm32 走 web-sys fetch，不存在 reqwest，本函数只在桌面/移动分支编译。
#[cfg(not(target_arch = "wasm32"))]
fn download_client() -> reqwest::Client {
    use std::sync::{Mutex, OnceLock};
    static C: OnceLock<Mutex<(u64, reqwest::Client)>> = OnceLock::new();
    let v = crate::network::proxy::current_version();
    let cell = C.get_or_init(|| Mutex::new((v, crate::network::proxy::build_download_client())));
    let mut g = cell.lock().unwrap();
    if g.0 != v {
        g.1 = crate::network::proxy::build_download_client();
        g.0 = v;
    }
    g.1.clone()
}

/// 获取歌曲音频字节：磁盘缓存命中直接返回；否则解析可播 URL（主源 → 多音源
/// 兜底）下载并写入缓存。
pub(crate) async fn fetch_song_bytes(
    client: &NeteaseClient,
    uid: i64,
    name: &str,
    artist: &str,
    dur_sec: u64,
) -> Option<Vec<u8>> {
    if let Some(b) = crate::persist::cache::load_song(uid) {
        return Some(b);
    }
    let http = download_client();

    // 主源：网易云直链（URL 解析 + 整首下载）
    let main = async {
        if let Ok(v) = crate::network::service::song_url(client, &[uid], 999000).await {
            if let Some(arr) = v.as_array() {
                for d in arr {
                    if let Some(u) = d.get("url").and_then(|x| x.as_str()) {
                        if u.is_empty() { continue; }
                        return try_download(&http, u).await;
                    }
                }
            }
        }
        None
    };
    // 兜底：延迟 4s 启动，避免主源正常时白跑探测；主源被限速/挂起时抢在前面完成
    let fallback = async {
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        if let Ok(v) = crate::network::service::resolve_fallback(name, artist, Some(dur_sec)).await {
            if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
                if !u.is_empty() {
                    return try_download(&http, u).await;
                }
            }
        }
        None
    };

    // 竞速：先成功者胜出；先完成的一侧为 None 时继续等另一侧
    let mut main = Box::pin(main);
    let mut fallback = Box::pin(fallback);
    let mut done: Option<Vec<u8>> = None;
    tokio::select! {
        r = &mut main => {
            done = r;
            if done.is_none() {
                done = fallback.await;
            }
        }
        r = &mut fallback => {
            done = r;
            if done.is_none() {
                done = main.await;
            }
        }
    }
    if let Some(b) = done {
        // 写盘 + 淘汰扫描进阻塞池，不在 async worker 上做整首音频 IO
        return crate::persist::cache::save_song_bg(uid, b).await;
    }

    // 洛雪音源脚本兜底（用户导入 .js，网易云直链 + 内置 7 源都失败后逐源解析）。
    // 仅桌面（rquickjs 为 desktop feature 依赖，移动端不编译）。
    #[cfg(feature = "desktop")]
    {
        if crate::network::sources::lx_scripts::enabled_any() {
            let info = crate::network::sources::lx_scripts::music_info(uid, name, artist, "", dur_sec);
            let quality = crate::persist::config::AppConfig::load().play.quality_index;
            if let Some(u) = crate::network::sources::lx_scripts::resolve(&info, quality).await {
                if let Some(b) = try_download(&http, &u).await {
                    return crate::persist::cache::save_song_bg(uid, b).await;
                }
            }
        }
    }
    None
}

/// 尝试从 URL 下载非空音频字节（下载失败/空内容视为不可播）。
/// 单源下载上限 20s：慢源超时即放弃并转下一个兜底源，
/// 避免用户长时间停在"加载中"（read_timeout 30s 对整首偏长）。
async fn try_download(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let fut = async {
        let resp = http.get(url).send().await.ok()?;
        let b = resp.bytes().await.ok()?;
        if b.is_empty() { None } else { Some(b.to_vec()) }
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), fut)
        .await
        .ok()
        .flatten()
}

// ===================== 流式播放（边下边播 + 边下边缓存）=====================

/// 播放音频来源：磁盘缓存命中直接给整首字节；否则给流式缓冲（首块到达即可出声）。
pub(crate) enum AudioSource {
    Bytes(Vec<u8>),
    Stream(std::sync::Arc<crate::core::stream::StreamBuffer>),
}

/// 打开音频来源（移动端播放用）：优先磁盘缓存；未命中则后台流式下载，
/// 首块数据到达立即返回，其余数据继续下载并写入缓存文件。
pub(crate) async fn open_audio(
    client: std::sync::Arc<NeteaseClient>,
    uid: i64,
    name: String,
    artist: String,
    dur_sec: u64,
) -> Option<AudioSource> {
    if let Some(b) = crate::persist::cache::load_song(uid) {
        return Some(AudioSource::Bytes(b));
    }
    let http = download_client();
    let buf = crate::core::stream::StreamBuffer::new();
    let task = {
        let buf = buf.clone();
        tokio::spawn(async move {
            stream_download(http, client, uid, name, artist, dur_sec, buf).await;
        })
    };
    // 等首块数据或下载结束（主源首块 6s 超时 + 兜底，总上限 20s）
    for _ in 0..2000 {
        if buf.len() > 0 {
            return Some(AudioSource::Stream(buf));
        }
        if buf.failed() || task.is_finished() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    if buf.len() > 0 {
        Some(AudioSource::Stream(buf))
    } else {
        None
    }
}

/// 后台流式下载：主源 → 多音源兜底（桌面另有洛雪脚本兜底），全程边下边写缓存。
async fn stream_download(
    http: reqwest::Client,
    client: std::sync::Arc<NeteaseClient>,
    uid: i64,
    name: String,
    artist: String,
    dur_sec: u64,
    buf: std::sync::Arc<crate::core::stream::StreamBuffer>,
) {
    let mut ok = false;
    if !buf.is_cancelled() {
        if let Ok(v) = crate::network::service::song_url(&client, &[uid], 999000).await {
            if let Some(arr) = v.as_array() {
                for d in arr {
                    if let Some(u) = d.get("url").and_then(|x| x.as_str()) {
                        if u.is_empty() {
                            continue;
                        }
                        ok = pump_url(&http, u, uid, &buf).await;
                        break;
                    }
                }
            }
        }
    }
    if !ok && !buf.is_cancelled() {
        if let Ok(v) = crate::network::service::resolve_fallback(&name, &artist, Some(dur_sec)).await {
            if let Some(u) = v.get("url").and_then(|x| x.as_str()) {
                if !u.is_empty() {
                    ok = pump_url(&http, u, uid, &buf).await;
                }
            }
        }
    }
    // 洛雪音源脚本兜底（仅桌面；移动端不编译 rquickjs）
    #[cfg(feature = "desktop")]
    if !ok && !buf.is_cancelled() && crate::network::sources::lx_scripts::enabled_any() {
        let info = crate::network::sources::lx_scripts::music_info(uid, &name, &artist, "", dur_sec);
        let quality = crate::persist::config::AppConfig::load().play.quality_index;
        if let Some(u) = crate::network::sources::lx_scripts::resolve(&info, quality).await {
            ok = pump_url(&http, &u, uid, &buf).await;
        }
    }
    if !ok && !buf.is_cancelled() {
        buf.fail();
        crate::persist::cache::abort_song_stream(uid);
    }
}

/// 从单个 URL 拉流：逐块写入缓冲与缓存临时文件。
/// 返回值表示"该源已被采用"（首块已到）；已采用后中途断开按失败处理（不换源重播）。
async fn pump_url(
    http: &reqwest::Client,
    url: &str,
    uid: i64,
    buf: &std::sync::Arc<crate::core::stream::StreamBuffer>,
) -> bool {
    use std::io::Write;
    let Ok(resp) = http.get(url).send().await else { return false };
    if !resp.status().is_success() {
        return false;
    }
    let mut resp = resp;
    let mut file = crate::persist::cache::open_song_stream(uid);
    let mut got = false;
    loop {
        if buf.is_cancelled() {
            drop(file.take());
            crate::persist::cache::abort_song_stream(uid);
            return got;
        }
        let next = if got {
            resp.chunk().await
        } else {
            // 首块 6s 超时视为该源不可用，交给下一个兜底源
            match tokio::time::timeout(std::time::Duration::from_secs(6), resp.chunk()).await {
                Ok(r) => r,
                Err(_) => {
                    drop(file.take());
                    crate::persist::cache::abort_song_stream(uid);
                    return false;
                }
            }
        };
        match next {
            Ok(Some(c)) => {
                if c.is_empty() {
                    continue;
                }
                got = true;
                buf.push(&c);
                if let Some(f) = file.as_mut() {
                    let _ = f.write_all(&c);
                }
            }
            Ok(None) => break,
            Err(_) => {
                drop(file.take());
                if got {
                    // 播放中途断流：标记失败（已出声，不换源重播）
                    buf.fail();
                    crate::persist::cache::abort_song_stream(uid);
                    return true;
                }
                crate::persist::cache::abort_song_stream(uid);
                return false;
            }
        }
    }
    drop(file.take());
    if got {
        buf.finish();
        crate::persist::cache::finish_song_stream(uid);
        true
    } else {
        crate::persist::cache::abort_song_stream(uid);
        false
    }
}

/// 解析搜索建议：优先 allMatch（新格式），回退到 songs/artists/albums/playlists 分类结构。
pub(crate) fn parse_suggest_items(v: &serde_json::Value) -> Vec<(String, String)> {
    let kind_of = |t: i64| -> &'static str {
        match t {
            1 => "song",
            100 => "artist",
            10 => "album",
            1000 => "playlist",
            1004 => "mv",
            1006 => "lyric",
            1009 => "radio",
            _ => "keyword",
        }
    };
    if let Some(arr) = v.pointer("/result/allMatch").and_then(|a| a.as_array()) {
        let list: Vec<(String, String)> = arr
            .iter()
            .filter_map(|m| {
                let k = m.get("keyword").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if k.is_empty() {
                    return None;
                }
                let t = m.get("type").and_then(|x| x.as_i64()).unwrap_or(0);
                Some((k, kind_of(t).to_string()))
            })
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    let mut out = Vec::new();
    for (key, kind) in [("songs", "song"), ("artists", "artist"), ("albums", "album"), ("playlists", "playlist")] {
        if let Some(arr) = v.pointer(&format!("/result/{}", key)).and_then(|a| a.as_array()) {
            for item in arr {
                let name = item.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if !name.is_empty() {
                    out.push((name, kind.to_string()));
                    if out.len() >= 8 {
                        break;
                    }
                }
            }
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}
