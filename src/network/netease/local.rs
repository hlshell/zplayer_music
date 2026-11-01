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

/// 递归扫描目录下的音频 + 视频文件。
/// 返回 (songs, videos) 两个 Vec<serde_json::Value>，结构同 scan_local_music 的单条。
pub fn scan_local_resources(dir: &str, recursive: bool) -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>), String> {
    let root = PathBuf::from(dir);
    if !root.exists() {
        return Err(format!("目录不存在: {}", dir));
    }
    if !root.is_dir() {
        return Err(format!("不是目录: {}", dir));
    }

    let mut songs = Vec::new();
    let mut videos = Vec::new();
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
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let file_name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let fallback_name = file_name.trim_end_matches(&format!(".{}", ext)).to_string();

            if is_audio(&path) {
                let (title, artist, album, duration) = probe_tags(&path, &fallback_name, &ext);
                songs.push(json!({
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
                }));
            } else if is_video(&path) {
                // 视频不解析元数据（避免阻塞），用文件名作标题
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
        sub_dirs.sort();
        stack.extend(sub_dirs.into_iter().rev());
    }

    songs.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));
    videos.sort_by(|a, b| a["name"].as_str().unwrap_or("").to_lowercase().cmp(&b["name"].as_str().unwrap_or("").to_lowercase()));

    Ok((songs, videos))
}
