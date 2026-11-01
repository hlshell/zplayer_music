//! 多音源兜底解析：当网易云无版权/VIP 拿不到可播 URL 时，
//! 依次尝试咪咕 → 酷我 → 酷狗 → QQ音乐 → YouTube → SoundCloud，
//! 用「歌名 + 歌手」在各平台搜索并取首个可播地址。

pub mod md5;
pub mod migu;
pub mod kuwo;
pub mod kugou;
pub mod qq;
pub mod joox;
pub mod youtube;
pub mod soundcloud;
pub mod gdmusic;
// 洛雪脚本执行器 / 脚本管理 / UNM：仅非 wasm（desktop/mobile）启用；
// web（wasm32）下 rquickjs / unm_engine 无法编译，由主源兜底。
#[cfg(not(target_arch = "wasm32"))]
pub mod lx;
#[cfg(not(target_arch = "wasm32"))]
pub mod lx_scripts;
#[cfg(not(target_arch = "wasm32"))]
pub mod unm;

use serde_json::Value;
use std::time::Duration;

/// 模拟前端 `<audio>` 加载外链资源时的请求特征：浏览器 UA、无 Referer/Origin、无 cookie。
/// （媒体元素跨域加载不触发 CORS，通常不带这些头；贴近它才能预示"前端可播"）
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

#[derive(Clone)]
pub struct MultiSourceClient {
    pub http: reqwest::Client,
}

impl MultiSourceClient {
    pub fn new() -> Self {
        let http = crate::network::proxy::apply(
            reqwest::Client::builder()
                .cookie_store(true)
                .timeout(Duration::from_secs(12))
                .connect_timeout(Duration::from_secs(8)),
        )
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }
}

/// 内容级可播性探测：过滤掉"后端能 200 但前端 audio 播不了"的源。
/// 判定标准贴近前端 audio 请求，而非后端最强的请求特征。
async fn probe_playable(c: &MultiSourceClient, url: &str) -> bool {
    let mut resp = match c
        .http
        .get(url)
        .header("User-Agent", BROWSER_UA)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !resp.status().is_success() {
        return false;
    }
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("audio/") {
        return true;
    }
    // 非标准 content-type（octet-stream/空/二进制），读首个数据块排除 HTML 错误页 / JSON 错误
    if ct.is_empty() || ct.contains("octet-stream") || ct.contains("binary") {
        if let Ok(Some(b)) = resp.chunk().await {
            if b.is_empty() {
                return false;
            }
            let s = String::from_utf8_lossy(&b);
            let t = s.trim_start();
            if t.starts_with("<!doctype") || t.starts_with("<html") || t.starts_with("{") || t.starts_with("[") {
                return false;
            }
            return true;
        }
        return false;
    }
    false
}

/// 一次兜底解析的产物
#[derive(serde::Serialize, Clone)]
pub struct ResolvedSong {
    pub url: String,
    pub source: &'static str,
    pub name: String,
    pub artist: String,
    pub platform_id: Option<String>,
    pub duration: Option<u64>,
}

/// 聚合搜索的歌曲条目（统一 schema，供前端全源并列展示）
#[derive(serde::Serialize, Clone)]
pub struct SourceSong {
    pub source: &'static str,
    pub id: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub cover: String,
    pub duration: Option<u64>,
}

/// 全源并列聚合搜索：并发查询各平台（咪咕/酷我/酷狗/QQ/YouTube/SoundCloud），
/// 按「歌名+歌手」归一化去重。网易云结果由调用方（commands.rs）追加并优先保留。
pub async fn search_all(keyword: &str) -> Vec<SourceSong> {
    let keyword = keyword.to_string();
    let mut results: Vec<SourceSong> = Vec::new();
    let mut tasks: Vec<tokio::task::JoinHandle<Vec<SourceSong>>> = Vec::new();
    for name in [migu::NAME, kuwo::NAME, kugou::NAME, qq::NAME, soundcloud::NAME] {
        let keyword = keyword.clone();
        let t = tokio::spawn(async move {
            let client = MultiSourceClient::new();
            let res = match name {
                migu::NAME => migu::search_songs(&client, &keyword).await,
                kuwo::NAME => kuwo::search_songs(&client, &keyword).await,
                kugou::NAME => kugou::search_songs(&client, &keyword).await,
                qq::NAME => qq::search_songs(&client, &keyword).await,
                youtube::NAME => youtube::search_songs(&client, &keyword).await,
                soundcloud::NAME => soundcloud::search_songs(&client, &keyword).await,
                _ => None,
            };
            res.unwrap_or_default()
        });
        tasks.push(t);
    }
    for t in tasks {
        if let Ok(list) = t.await {
            results.extend(list);
        }
    }
    dedup_songs(&mut results);
    results
}

/// 点阵式分组：同一首歌（归一化歌名+歌手）的多源并列，保留所有来源版本，供前端「一首歌多个源」展示
#[derive(serde::Serialize, Clone)]
pub struct SongVariant {
    pub source: String,
    pub id: String,
    pub duration: Option<u64>,
}
#[derive(serde::Serialize, Clone)]
pub struct GroupedSong {
    pub name: String,
    pub artist: String,
    pub album: String,
    pub cover: String,
    pub variants: Vec<SongVariant>,
}
/// 按「歌名+歌手」归一化分组：保留每个来源的版本（variants），主字段取第一个出现（网易云优先）
pub fn group_songs(list: Vec<SourceSong>) -> Vec<GroupedSong> {
    let mut out: Vec<GroupedSong> = Vec::new();
    let mut idx: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for s in list {
        let key = format!("{}|{}", norm(s.name.as_str()), norm(s.artist.as_str()));
        if let Some(&i) = idx.get(&key) {
            out[i].variants.push(SongVariant { source: s.source.to_string(), id: s.id, duration: s.duration });
        } else {
            idx.insert(key, out.len());
            out.push(GroupedSong {
                name: s.name,
                artist: s.artist,
                album: s.album,
                cover: s.cover,
                variants: vec![SongVariant { source: s.source.to_string(), id: s.id, duration: s.duration }],
            });
        }
    }
    out
}

/// 轮询贪心混排：分组后的歌曲按「主源」分桶，循环轮流从各源取 Top 剩余歌曲
/// （每源内部已按相关性排序；轮询保证来源均衡，不让单一源霸屏前部）
pub fn round_robin_mix(groups: Vec<GroupedSong>) -> Vec<GroupedSong> {
    const ORDER: [&str; 6] = ["netease", "migu", "kuwo", "kugou", "qq", "soundcloud"];
    let mut buckets: std::collections::HashMap<String, std::collections::VecDeque<GroupedSong>> = std::collections::HashMap::new();
    for g in groups {
        let src = g.variants.first().map(|v| v.source.clone()).unwrap_or_else(|| "netease".to_string());
        buckets.entry(src).or_default().push_back(g);
    }
    let mut out: Vec<GroupedSong> = Vec::new();
    loop {
        let mut moved = false;
        for src in ORDER {
            if let Some(b) = buckets.get_mut(src) {
                if let Some(g) = b.pop_front() {
                    out.push(g);
                    moved = true;
                }
            }
        }
        if !moved { break; }
    }
    out
}

/// 按源解析可播 URL（聚合搜索条目的播放入口）
pub async fn resolve_source(source: &str, id: &str, name: &str, artist: &str) -> Option<String> {
    let client = MultiSourceClient::new();
    if id.is_empty() {
        // 前端音源记忆逐源试：id 为空时按标题+歌手搜索取可播 URL
        let s = match source {
            migu::NAME => migu::try_resolve(&client, name, artist).await,
            kuwo::NAME => kuwo::try_resolve(&client, name, artist).await,
            kugou::NAME => kugou::try_resolve(&client, name, artist).await,
            qq::NAME => qq::try_resolve(&client, name, artist).await,
            joox::NAME => joox::try_resolve(&client, name, artist).await,
            youtube::NAME => youtube::try_resolve(&client, name, artist).await,
            soundcloud::NAME => soundcloud::try_resolve(&client, name, artist).await,
            _ => None,
        };
        return s.map(|x| x.url);
    }
    match source {
        migu::NAME => migu::resolve_url_by_id(&client, id, name, artist).await,
        kuwo::NAME => kuwo::resolve_url_by_id(&client, id, name, artist).await,
        kugou::NAME => kugou::resolve_url_by_id(&client, id, name, artist).await,
        qq::NAME => qq::resolve_url_by_id(&client, id, name, artist).await,
        joox::NAME => joox::resolve_url_by_id(&client, id, name, artist).await,
        youtube::NAME => youtube::resolve_url_by_id(&client, id, name, artist).await,
        soundcloud::NAME => soundcloud::resolve_url_by_id(&client, id, name, artist).await,
        _ => None,
    }
}

/// 按「歌名+歌手」归一化去重，保留第一个出现的（调用方保证网易云在前）
pub(crate) fn dedup_songs(list: &mut Vec<SourceSong>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|s| {
        let key = format!(
            "{}|{}",
            norm(s.name.as_str()),
            norm(s.artist.as_str())
        );
        seen.insert(key)
    });
}

/// 归一化时剔除的常见噪音词（官方视频/MV/Lyrics/高清 等），提升跨源同一首歌的匹配率
const NOISE_WORDS: &[&str] = &[
    "official", "video", "mv", "lyrics", "lyric", "hq", "hd", "audio", "karaoke",
    "live", "remaster", "officialvideo", "officialmv", "audioonly", "4k",
    "官方", "高清", "现场", "原版", "歌词", "原唱", "伴奏",
];
fn norm(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphabetic() || c.is_ascii_digit() { c.to_ascii_lowercase() } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| !NOISE_WORDS.contains(&w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按优先级遍历各平台，返回「内容级可播 + 时长最接近原歌」的结果
/// duration = 网易云原歌时长（秒）；源返回时长与原歌差异 >15s 视为匹配错，跳过
pub async fn resolve(title: &str, artist: &str, duration: Option<u64>) -> Result<Option<ResolvedSong>, String> {
    let client = MultiSourceClient::new();
    let mut best: Option<(ResolvedSong, u64)> = None; // (song, diff)；diff=u64::MAX 表示源未给时长
    // 主源列表：纯 HTTP 源（web 可编译）在首位；UNM 仅非 wasm 追加（web 下 rquickjs/unm 不可用）
    let mut sources = vec![
        migu::NAME, kuwo::NAME, kugou::NAME, qq::NAME, joox::NAME, youtube::NAME, soundcloud::NAME, gdmusic::NAME,
    ];
    #[cfg(not(target_arch = "wasm32"))]
    sources.push(unm::NAME);
    for src in sources {
        let song = match src {
            migu::NAME => migu::try_resolve(&client, title, artist).await,
            kuwo::NAME => kuwo::try_resolve(&client, title, artist).await,
            kugou::NAME => kugou::try_resolve(&client, title, artist).await,
            qq::NAME => qq::try_resolve(&client, title, artist).await,
            joox::NAME => joox::try_resolve(&client, title, artist).await,
            youtube::NAME => youtube::try_resolve(&client, title, artist).await,
            soundcloud::NAME => soundcloud::try_resolve(&client, title, artist).await,
            gdmusic::NAME => gdmusic::try_resolve(&client, title, artist).await,
            #[cfg(not(target_arch = "wasm32"))]
            unm::NAME => unm::try_resolve(&client, title, artist).await,
            _ => None,
        };
        if let Some(s) = song {
            // 方案B：内容级可播性探测，过滤"后端能取到但前端 audio 播不了"的源
            if !probe_playable(&client, &s.url).await {
                continue;
            }
            let diff = match (duration, s.duration) {
                (Some(nd), Some(sd)) => {
                    let d = nd.abs_diff(sd);
                    // 放宽时长差：>60s 才判为无关（翻唱/错误版本）；15s 内最佳，
                    // 15~60s 仍作低优先级候选，避免"能播的歌"被时长差误杀导致兜底失败
                    if d > 60 { u64::MAX - 1 } else { d }
                }
                _ => u64::MAX,
            };
            let better = match &best {
                None => true,
                Some((_, bd)) => diff < *bd,
            };
            if better {
                best = Some((s, diff));
            }
        }
    }
    Ok(best.map(|(s, _)| s))
}

/// 兜底入口（供 command 调用）：title/artist/duration 遍历平台，时长校验选最接近源
pub async fn resolve_fallback(title: &str, artist: &str, duration: Option<u64>) -> Result<Value, String> {
    match resolve(title, artist, duration).await? {
        Some(s) => Ok(serde_json::json!(s)),
        None => Ok(serde_json::json!({ "url": null, "source": null })),
    }
}

impl Default for MultiSourceClient {
    fn default() -> Self {
        Self::new()
    }
}

// 依赖 rquickjs / unm_engine 的地基验证，仅非 wasm（wasm 下这些依赖不编译）
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    // 地基验证：rquickjs(QuickJS) 在 Windows 编译 + 基本脚本执行
    #[test]
    fn quickjs_min() {
        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            let v: i64 = ctx.eval("1 + 2").unwrap();
            assert_eq!(v, 3);
            let s: String = ctx.eval("'晴天'.concat('周杰伦')").unwrap();
            assert_eq!(s, "晴天周杰伦");
        });
    }

    // 地基验证：UnblockNeteaseMusic（unm_engine Rust 版）能编译 + 注册引擎
    #[test]
    fn unm_basic_compile() {
        use std::borrow::Cow;
        use std::sync::Arc;
        let mut executor = unm_engine::executor::Executor::new();
        executor.register(
            Cow::Borrowed(unm_engine_kuwo::ENGINE_ID),
            Arc::new(unm_engine_kuwo::KuwoEngine),
        );
        assert!(!executor.list().is_empty());
    }
}
