//! 缓存管理：封面（URL 哈希命名）+ 歌曲（song_id 命名）磁盘缓存，
//! 以及容量限制清理（启动与写盘后按文件 mtime 混合清理最旧文件）。
//!
//! 统计范围：默认 cache/ 根目录 + 用户自定义的 cover-dir / song-dir（去重），
//! 封面与歌曲共用同一容量上限，谁旧先清谁。
//!
//! 目录/上限通过 `init(&AppConfig)` 注入为全局设置，便于在异步任务中直接调用
//! （避免把非 Send 的配置状态传入 tokio）。

use crate::persist::config::{self, AppConfig};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

/// 上次执行容量清理的时间戳（秒）。save_cover 每写一张封面都会调 enforce_limit，
/// 而单次清理 = 全目录遍历 + mtime 排序 + 批量删除；缓存贴近上限时几十张封面
/// 并发下载会同时跑几十次扫描（实测每秒数十条清理日志、单次删数十文件），
/// 磁盘 I/O 被占满，界面卡顿。用时间戳节流（原子 swap 保证并发下只有一个真执行）。
static LAST_ENFORCE: AtomicU64 = AtomicU64::new(0);

/// 封面类型：按类型分子目录存放（cover/<type>/<hash>），便于分类管理与清理。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CoverType {
    Song, Playlist, Album, Mv, Radio, Artist,
}
impl CoverType {
    pub fn subdir(&self) -> &'static str {
        match self {
            CoverType::Song => "song",
            CoverType::Playlist => "playlist",
            CoverType::Album => "album",
            CoverType::Mv => "mv",
            CoverType::Radio => "radio",
            CoverType::Artist => "artist",
        }
    }
}

struct CacheSettings {
    cover_dir: PathBuf,
    song_dir: PathBuf,
    lyric_dir: PathBuf,
    limit_mb: u64,
    /// 清理策略：0=最近少用(LRU) 1=先进先出(FIFO)
    evict_strategy: i32,
}

static SETTINGS: Mutex<Option<CacheSettings>> = Mutex::new(None);

/// 用配置初始化（应用启动 / 设置变更时调用）。
pub fn init(cfg: &AppConfig) {
    let s = CacheSettings {
        cover_dir: config::cover_cache_dir(cfg),
        song_dir: config::song_cache_dir(cfg),
        lyric_dir: config::lyric_cache_dir(cfg),
        limit_mb: cfg.app.cache_limit_mb,
        evict_strategy: cfg.cache.evict_strategy,
    };
    *SETTINGS.lock().unwrap() = Some(s);
}

/// 读取当前缓存设置；未初始化时回退默认值。
/// (cover, song, lyric, limit_mb, strategy)
fn settings() -> (PathBuf, PathBuf, PathBuf, u64, i32) {
    let g = SETTINGS.lock().unwrap();
    match g.as_ref() {
        Some(s) => (
            s.cover_dir.clone(),
            s.song_dir.clone(),
            s.lyric_dir.clone(),
            s.limit_mb,
            s.evict_strategy,
        ),
        None => (
            config::cache_dir().join("covers"),
            config::cache_dir().join("songs"),
            config::cache_dir().join("lyrics"),
            0,
            0,
        ),
    }
}

/// URL → 16 位 hex 短哈希（DefaultHasher，碰撞对封面缓存无害）。
fn hash(url: &str) -> String {
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 封面缓存文件路径（按类型子目录）。旧的 legacy 平铺目录在 load 时兜底。
fn cover_file(url: &str, t: CoverType) -> PathBuf {
    let (dir, _, _, _, _) = settings();
    dir.join(t.subdir()).join(hash(url))
}

/// 旧版本平铺封面路径（未分类，迁移后存放于 legacy/ 供兼容命中）。
fn legacy_cover_file(url: &str) -> PathBuf {
    let (dir, _, _, _, _) = settings();
    dir.join("legacy").join(hash(url))
}

/// 歌曲缓存文件路径。
fn song_file(id: i64) -> PathBuf {
    let (_, dir, _, _, _) = settings();
    dir.join(id.to_string())
}

/// 读取封面缓存（命中返回原始字节）。先查类型子目录，未命中回退 legacy（兼容旧缓存）。
pub fn load_cover(url: &str, t: CoverType) -> Option<Vec<u8>> {
    std::fs::read(cover_file(url, t)).ok()
        .or_else(|| std::fs::read(legacy_cover_file(url)).ok())
}

/// 写入封面缓存（下载成功后调用），并触发容量清理。
pub fn save_cover(url: &str, t: CoverType, bytes: &[u8]) {
    let p = cover_file(url, t);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&p, bytes).is_ok() {
        enforce_limit();
    }
}

/// 把旧版平铺在 cover 根目录下的封面文件迁移到 legacy/ 子目录（启动时调用）。
/// 迁移无法从哈希反推类型，故统一归入 legacy，由 load_cover 兜底命中；新缓存按类型写入。
pub fn migrate_legacy_covers() {
    let (dir, _, _, _, _) = settings();
    let legacy = dir.join("legacy");
    if !legacy.exists() {
        let _ = std::fs::create_dir_all(&legacy);
    }
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                let _ = std::fs::rename(&p, legacy.join(p.file_name().unwrap_or_default()));
            }
        }
    }
}

/// 读取歌曲缓存。
pub fn load_song(id: i64) -> Option<Vec<u8>> {
    std::fs::read(song_file(id)).ok()
}

/// 写入歌曲缓存（播放下载成功后调用），并触发容量清理。
pub fn save_song(id: i64, bytes: &[u8]) {
    let p = song_file(id);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::write(&p, bytes).is_ok() {
        enforce_limit();
    }
}

/// 取歌路径专用：把"写整首音频 + 淘汰扫描"移出 tokio 异步 worker（阻塞池执行），
/// 并原样回传字节（move 进出，零拷贝），调用方拿到后可直接送解码播放。
/// 返回 None 仅出现在阻塞任务 panic 的极端情况（写盘错误已在 save_song 内部吞掉）。
pub async fn save_song_bg(id: i64, bytes: Vec<u8>) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        save_song(id, &bytes);
        bytes
    })
    .await
    .ok()
}

/// 参与统计/清理的目录集合（默认 cache 根 + 歌曲/封面/歌词目录，去重）。
fn tracked_dirs() -> Vec<PathBuf> {
    let (cover, song, lyric, _, _) = settings();
    let mut dirs = vec![config::cache_dir()];
    for d in [cover, song, lyric] {
        if !dirs.iter().any(|x| x == &d) {
            dirs.push(d);
        }
    }
    dirs
}

/// 递归收集目录下所有文件（路径, 字节数, mtime 秒）。
/// mtime 必须在此处一次性读好并随条目携带：若排序时现读（sort_by_key(|p| mtime(p))），
/// 并发的清理线程删文件会让 key 值在排序过程中变化，触发 std 的
/// "comparison function does not correctly implement a total order" panic（实测崩溃）。
fn collect_files(dir: &Path, out: &mut Vec<(PathBuf, u64, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, out);
        } else if let Ok(md) = e.metadata() {
            let mt = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            out.push((p, md.len(), mt));
        }
    }
}

/// 当前缓存总占用（字节）。
pub fn total_usage() -> u64 {
    let mut files = Vec::new();
    for d in tracked_dirs() {
        collect_files(&d, &mut files);
    }
    files.iter().map(|(_, s, _)| s).sum()
}

/// 容量限制：超限时按策略删除文件，直到低于上限。
/// limit_mb == 0 表示不限制。
/// 策略：0=LRU（按 mtime 升序，最旧优先）；1=FIFO（按文件名升序，先进先出）。
///
/// 写盘路径（save_cover / save_song）每张文件都会调用，因此此处做 10s 节流：
/// 一次清理要全目录遍历 + 排序 + 删一批文件，几十张封面并发落盘时若各自扫描一遍，
/// 磁盘 I/O 会被占满（实测每秒数十次、单次删数十文件，界面明显卡顿）。
pub fn enforce_limit() {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // swap 后只有"距上次执行 >=10s"的那个调用会看到旧时间戳并继续往下执行
    let prev = LAST_ENFORCE.swap(now, Ordering::Relaxed);
    if now.saturating_sub(prev) < 10 {
        return;
    }
    enforce_limit_inner();
}

/// 立即执行容量清理（设置页手动触发的场景，不受 10s 节流限制）。
pub fn enforce_limit_now() {
    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    LAST_ENFORCE.store(now, Ordering::Relaxed);
    enforce_limit_inner();
}

fn enforce_limit_inner() {
    let (_, _, _, limit_mb, strategy) = settings();
    if limit_mb == 0 {
        return;
    }
    let limit = limit_mb.saturating_mul(1024 * 1024);
    // 删到 90% 水位而非刚好卡在上限：否则清理完立刻被新下载填满，
    // 一直贴着上限来回触发清理。留出余量可显著降低清理频率。
    let target = limit / 10 * 9;
    let mut files = Vec::new();
    for d in tracked_dirs() {
        collect_files(&d, &mut files);
    }
    let mut total: u64 = files.iter().map(|(_, s, _)| s).sum();
    if total <= limit {
        return;
    }
    if strategy == 1 {
        files.sort_by_key(|(p, _, _)| p.file_name().map(|n| n.to_os_string()).unwrap_or_default());
    } else {
        // 用收集时快照的 mtime 排序（不再实时读盘，见 collect_files 注释）
        files.sort_by_key(|(_, _, m)| *m);
    }
    let mut removed = 0u64;
    for (p, size, _) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(size);
            removed += 1;
        }
    }
    log::info!(
        "[cache] 容量限制 {}MB（策略 {}），已清理 {} 个文件，当前占用 {:.1}MB",
        limit_mb,
        strategy,
        removed,
        total as f64 / (1024.0 * 1024.0)
    );
}

// ================= 统计与分类清理（设置页「缓存状态 / 缓存管理」）=================

/// 统计某目录下的文件数。
fn count_files(dir: &Path) -> u64 {
    let mut n = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

/// 音乐缓存文件数（歌曲缓存目录）。
pub fn count_music() -> u64 {
    let (_, dir, _, _, _) = settings();
    count_files(&dir)
}

/// 封面缓存文件数（封面缓存目录）。
pub fn count_covers() -> u64 {
    let (dir, _, _, _, _) = settings();
    count_files(&dir)
}

/// 歌词缓存文件数（歌词缓存目录）。
pub fn count_lyrics() -> u64 {
    let (_, _, dir, _, _) = settings();
    count_files(&dir)
}

/// 删除目录内全部文件（保留子目录结构）。
fn clear_dir(dir: &Path) -> u64 {
    let mut removed = 0u64;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                removed += clear_dir(&p);
            } else if std::fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// 清理音乐缓存。
pub fn clear_music() -> u64 {
    let (_, dir, _, _, _) = settings();
    clear_dir(&dir)
}

/// 清理封面缓存。
pub fn clear_covers() -> u64 {
    let (dir, _, _, _, _) = settings();
    clear_dir(&dir)
}

/// 清理歌词缓存。
pub fn clear_lyrics() -> u64 {
    let (_, _, dir, _, _) = settings();
    clear_dir(&dir)
}

/// 清理全部缓存（音乐+歌词+封面+默认目录）。
pub fn clear_all() -> u64 {
    let mut removed = 0u64;
    for d in tracked_dirs() {
        removed += clear_dir(&d);
    }
    removed
}
