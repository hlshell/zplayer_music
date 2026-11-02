//! 洛雪音源脚本管理：脚本目录扫描 / 手动导入 / 启用列表持久化 /
//! 播放兜底逐源解析（getMusicUrl → 内容级可播探测）。
//!
//! 脚本存放于跨平台数据目录 `ZPlayerMusic/lx-scripts/`（每个 .js 一个音源脚本，
//! 内含 `musicSources` 对象）。启用列表写入 config.toml 的 `[lx]`，兜底只遍历
//! 启用脚本。

use crate::network::sources::lx::LxSourceRunner;
use crate::persist::config::{data_dir, AppConfig};

/// 音源脚本目录（跨平台数据目录下 lx-scripts/）。
pub fn scripts_dir() -> std::path::PathBuf {
    data_dir().join("lx-scripts")
}

/// 磁盘上所有 .js 脚本文件名（未启用/已启用一并列出，供设置页展示）。
pub fn list_scripts() -> Vec<String> {
    let dir = scripts_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".js") {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// 手动导入：把外部 .js 复制到脚本目录（同名则覆盖）。
pub fn import_script(src: &std::path::Path) -> Result<String, String> {
    let dir = scripts_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建脚本目录失败: {e}"))?;
    let name = src
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    if !name.ends_with(".js") {
        return Err("仅支持 .js 音源脚本".into());
    }
    let dst = dir.join(&name);
    std::fs::copy(src, &dst).map_err(|e| format!("复制脚本失败: {e}"))?;
    // 导入后立即校验脚本能否初始化
    let text = std::fs::read_to_string(&dst).map_err(|e| format!("读取脚本失败: {e}"))?;
    LxSourceRunner::new(&text).map_err(|e| format!("脚本初始化失败: {e}"))?;
    Ok(name)
}

/// 构造传给音源脚本的歌曲信息（对齐 old convertToLxMusicInfo 的字段）。
pub fn music_info(
    id: i64,
    name: &str,
    singer: &str,
    album: &str,
    duration_sec: u64,
) -> serde_json::Value {
    serde_json::json!({
        "songmid": id.to_string(),
        "name": name,
        "singer": singer,
        "album": album,
        "albumId": "",
        "source": "wy",
        "interval": duration_sec,
        "img": ""
    })
}

/// 由配置映射音质到脚本约定值（0=标准 1=高音质 2=无损）。
fn quality_str(idx: i32) -> &'static str {
    match idx {
        1 => "320k",
        2 => "flac",
        _ => "128k",
    }
}

/// 内置维护清单：pdone/lx-music-source 的可用音源（截至 2026-10-21 实时检测 🟢）。
/// 每项 = (脚本文件名, GitHub raw 直链, ghproxy 加速直链)。
/// 失效/异常源（flower/grass/ikun/huibq/juhe/lx）默认不纳入，避免首次拉取即失败。
const BUILTIN_SOURCES: &[(&str, &str, &str)] = &[
    (
        "changqing.js",
        "https://raw.githubusercontent.com/pdone/lx-music-source/main/changqing/latest.js",
        "https://ghproxy.net/raw.githubusercontent.com/pdone/lx-music-source/main/changqing/latest.js",
    ),
    (
        "huanyin.js",
        "https://raw.githubusercontent.com/pdone/lx-music-source/main/huanyin/latest.js",
        "https://ghproxy.net/raw.githubusercontent.com/pdone/lx-music-source/main/huanyin/latest.js",
    ),
    (
        "qdy.js",
        "https://raw.githubusercontent.com/pdone/lx-music-source/main/qdy/latest.js",
        "https://ghproxy.net/raw.githubusercontent.com/pdone/lx-music-source/main/qdy/latest.js",
    ),
    (
        "xinghai.js",
        "https://raw.githubusercontent.com/pdone/lx-music-source/main/xinghai/latest.js",
        "https://ghproxy.net/raw.githubusercontent.com/pdone/lx-music-source/main/xinghai/latest.js",
    ),
    (
        "sixyin.js",
        "https://raw.githubusercontent.com/pdone/lx-music-source/main/sixyin/latest.js",
        "https://ghproxy.net/raw.githubusercontent.com/pdone/lx-music-source/main/sixyin/latest.js",
    ),
];

/// 更新报告
pub struct UpdateReport {
    pub added: Vec<String>,
    pub failed: Vec<String>,
}

/// 下载单个文本（先 raw，失败用 ghproxy 加速，两者都失败返回 None）。
async fn fetch_text(http: &reqwest::Client, url: &str) -> Option<String> {
    let r = http.get(url).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.text().await.ok()
}

/// 从内置维护清单拉取/更新音源脚本：逐源 GET → 校验可初始化 → 写入脚本目录
/// 并加入启用列表（自动更新 / 首次内置默认共用）。
pub async fn update_from_builtin() -> Result<UpdateReport, String> {
    let http = crate::network::proxy::build_client(None);
    let mut cfg = AppConfig::load();
    let mut added = Vec::new();
    let mut failed = Vec::new();
    for (file, raw, accel) in BUILTIN_SOURCES {
        let mut text = fetch_text(&http, raw).await;
        if text.is_none() {
            text = fetch_text(&http, accel).await;
        }
        let Some(t) = text else {
            failed.push(file.to_string());
            continue;
        };
        // 脚本能初始化（musicSources 存在、无语法错误）才写入启用
        if LxSourceRunner::new(&t).is_ok() {
            let dir = scripts_dir();
            let _ = std::fs::create_dir_all(&dir);
            if std::fs::write(dir.join(file), &t).is_ok() {
                if !cfg.lx.enabled.contains(&file.to_string()) {
                    cfg.lx.enabled.push(file.to_string());
                }
                added.push(file.to_string());
            } else {
                failed.push(file.to_string());
            }
        } else {
            failed.push(file.to_string());
        }
    }
    cfg.save();
    Ok(UpdateReport { added, failed })
}

/// 启用脚本数（判断是否需要初始化内置默认）。
pub fn enabled_count() -> usize {
    AppConfig::load().lx.enabled.len()
}

/// 是否启用任何洛雪脚本（避免空跑解析）。
pub fn enabled_any() -> bool {
    let cfg = AppConfig::load();
    cfg.lx.fallback_enabled && !cfg.lx.enabled.is_empty()
}

/// 读取启用列表（脚本文件名）。
pub fn enabled_list() -> Vec<String> {
    AppConfig::load().lx.enabled
}

/// 更新启用列表并持久化。
pub fn set_enabled(enabled: Vec<String>) {
    let mut cfg = AppConfig::load();
    cfg.lx.enabled = enabled;
    cfg.save();
}

/// 设置洛雪兜底开关并持久化。
pub fn set_fallback_enabled(on: bool) {
    let mut cfg = AppConfig::load();
    cfg.lx.fallback_enabled = on;
    cfg.save();
}

/// 按启用脚本逐源解析可播 URL：遍历启用脚本 → 该脚本的每个音源
/// getMusicUrl → 内容级可播探测 → 返回第一个可播直链。
pub async fn resolve(info: &serde_json::Value, quality_index: i32) -> Option<String> {
    let cfg = AppConfig::load();
    if !cfg.lx.fallback_enabled {
        return None;
    }
    let quality = quality_str(quality_index);
    for script_name in &cfg.lx.enabled {
        let path = scripts_dir().join(script_name);
        let Ok(text) = std::fs::read_to_string(&path) else { continue; };
        let Ok(runner) = LxSourceRunner::new(&text) else { continue; };
        for source in runner.sources() {
            if let Some(url) = runner.get_music_url(source, info, quality) {
                if url.is_empty() {
                    continue;
                }
                // 内容级可播探测：能取到非空音频字节才认可用
                let client = super::MultiSourceClient::new();
                if super::probe_playable(&client, &url).await {
                    return Some(url);
                }
            }
        }
    }
    None
}
