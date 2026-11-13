//! 本地资源扫描（离线真实数据 + 音频标签解析）
//! 扫描目录下的音频与视频文件，供"本地资源"页展示与播放。

use lofty::prelude::*;
use serde_json::json;
use std::path::{Path, PathBuf};

const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac", "ape", "opus", "wma", "aiff", "alac"];
const VIDEO_EXTS: &[&str] = &["mp4", "mkv", "avi", "mov", "flv", "wmv", "webm", "m4v", "mpg", "mpeg", "ts", "3gp"];

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 用 lofty 解析单个音频文件的元数据（失败时回退到文件名）
fn probe_tags(path: &Path, fallback_name: &str, ext: &str) -> (String, String, String, u64) {
    let (mut title, mut artist, mut album) = (fallback_name.to_string(), String::new(), String::new());
    let mut duration = 0u64;
    if let Ok(tagged) = lofty::probe::Probe::open(path).and_then(|p| p.read()) {
        if let Some(tag) = tagged.primary_tag() {
            title = tag.title().map(|c| c.into_owned()).unwrap_or_else(|| fallback_name.to_string());
            artist = tag.artist().map(|c| c.into_owned()).unwrap_or_default();
            album = tag.album().map(|c| c.into_owned()).unwrap_or_default();
        } else if let Some(tag) = tagged.first_tag() {
            title = tag.title().map(|c| c.into_owned()).unwrap_or_else(|| fallback_name.to_string());
            artist = tag.artist().map(|c| c.into_owned()).unwrap_or_default();
            album = tag.album().map(|c| c.into_owned()).unwrap_or_default();
        }
        duration = tagged.properties().duration().as_secs();
    }
    if title.is_empty() {
        title = fallback_name.to_string();
    }
    // 有些文件 duration 为 0，保留 0 由前端显示
    let _ = ext;
    (title, artist, album, duration)
}

/// 递归扫描目录下的音频文件，返回元信息（路径/标题/艺术家/专辑/时长/格式/大小）
pub fn scan_local_music(dir: &str, recursive: bool) -> Result<serde_json::Value, String> {
    let root = PathBuf::from(dir);
    if !root.exists() {
        return Err(format!("目录不存在: {}", dir));
    }
    if !root.is_dir() {
        return Err(format!("不是目录: {}", dir));
    }

    let mut songs = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];

    while let Some(dir_path) = stack.pop() {
        let entries = std::fs::read_dir(&dir_path)
            .map_err(|e| format!("读取目录 {} 失败: {}", dir_path.display(), e))?;
        let mut sub_dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recursive {
                    sub_dirs.push(path);
                }
                continue;
            }
            if is_audio(&path) {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let file_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let fallback_name = file_name.trim_end_matches(&format!(".{}", ext)).to_string();
                let (title, artist, album, duration) = probe_tags(&path, &fallback_name, &ext);
                songs.push(json!({
                    "id": 0,
                    "path": path.to_string_lossy(),
                    "name": title,
                    "title": title,
                    "artist": artist,
                    "album": album,
                    "fileName": file_name,
                    "format": ext,
                    "size": size,
                    "duration": duration,
                    "local": true
                }));
            }
        }
        // 后进先出，保证稳定顺序
        sub_dirs.sort();
        stack.extend(sub_dirs.into_iter().rev());
    }

    songs.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b["name"].as_str().unwrap_or("").to_lowercase())
    });

    Ok(json!({
        "dir": root.to_string_lossy(),
        "total": songs.len(),
        "songs": songs,
    }))
}

/// 递归收集目录下所有文件路径（不读元数据，先分类再按需 stat）。
/// 子目录不可读时跳过并记录日志，避免单个权限问题导致整次扫描失败。
fn walk_files(root: &Path, recursive: bool) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir_path) = stack.pop() {
        let entries = match std::fs::read_dir(&dir_path) {
            Ok(e) => e,
            Err(e) if dir_path.as_path() == root => {
                return Err(format!("读取目录 {} 失败: {}", dir_path.display(), e));
            }
            Err(e) => {
                log::warn!(target: "local", "跳过不可读目录 {}: {}", dir_path.display(), e);
                continue;
            }
        };
        let mut sub_dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if recursive {
                    sub_dirs.push(path);
                }
            } else {
                out.push(path);
            }
        }
        // 后进先出，保证稳定顺序
        sub_dirs.sort();
        stack.extend(sub_dirs.into_iter().rev());
    }
    Ok(out)
}

/// 递归扫描目录下的音频 + 视频文件。
/// 返回 (songs, videos) 两个 Vec<serde_json::Value>，结构同 scan_local_music 的单条。
///
/// 性能要点（大目录下差异显著）：
/// - 先遍历收集路径，只对媒体文件做 metadata（原实现每个文件都 stat 一次）；
/// - 音频标签解析按 CPU 并行度切块多线程执行（lofty 解析是扫描的主要耗时）；
/// - 跳过不可读子目录而不是整体失败。
pub fn scan_local_resources(dir: &str, recursive: bool) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), String> {
    let root = PathBuf::from(dir);
    if !root.exists() {
        return Err(format!("目录不存在: {}", dir));
    }
    if !root.is_dir() {
        return Err(format!("不是目录: {}", dir));
    }

    let mut videos = Vec::new();
    let mut audio: Vec<(PathBuf, String, String, u64)> = Vec::new();
    for path in walk_files(&root, recursive)? {
        let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let is_audio_file = is_audio(&path);
        let is_video_file = is_video(&path);
        if !is_audio_file && !is_video_file {
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if is_audio_file {
            audio.push((path, file_name, ext, size));
        } else {
            // 视频不解析元数据（避免阻塞），用文件名作标题
            let fallback_name = file_name.trim_end_matches(&format!(".{}", ext)).to_string();
            videos.push(json!({
                "id": 0,
                "path": path.to_string_lossy(),
                "name": fallback_name,
                "artist": "",
                "album": "",
                "fileName": file_name,
                "format": ext,
                "size": size,
                "duration": 0,
                "local": true
            }));
        }
    }

    // 音频标签解析并行化：按 CPU 数切块，各线程独立处理自己的片段（lofty 无共享状态）。
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = audio.len().div_ceil(workers).max(1);
    let mut songs: Vec<serde_json::Value> = Vec::with_capacity(audio.len());
    std::thread::scope(|s| {
        let mut handles = Vec::new();
        for part in audio.chunks(chunk) {
            handles.push(s.spawn(move || {
                part.iter()
                    .map(|(path, file_name, ext, size)| {
                        let fallback_name = file_name.trim_end_matches(&format!(".{}", ext)).to_string();
                        let (title, artist, album, duration) = probe_tags(path, &fallback_name, ext);
                        json!({
                            "id": 0,
                            "path": path.to_string_lossy(),
                            "name": title,
                            "artist": artist,
                            "album": album,
                            "fileName": file_name,
                            "format": ext,
                            "size": size,
                            "duration": duration,
                            "local": true
                        })
                    })
                    .collect::<Vec<_>>()
            }));
        }
        for h in handles {
            if let Ok(v) = h.join() {
                songs.extend(v);
            }
        }
    });

    songs.sort_by_cached_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());
    videos.sort_by_cached_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());

    Ok((songs, videos))
}
