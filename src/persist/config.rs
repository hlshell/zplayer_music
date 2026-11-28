//! 应用配置：跨平台数据目录 + config.toml 读写。
//! 统一管理：主题 / 窗口位置 / 音量 / 播放记忆 / 缓存设置。
//!
//! 目录布局（dirs crate 跨平台路由）：
//!   Windows: %APPDATA%\ZPlayerMusic
//!   macOS:   ~/Library/Application Support/ZPlayerMusic
//!   Linux:   ~/.local/share/ZPlayerMusic
//!
//!   ZPlayerMusic/
//!   ├── config.toml      # 本文件
//!   ├── cache/covers/    # 封面缓存
//!   ├── cache/songs/     # 歌曲缓存
//!   └── log/             # 运行日志

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

/// 平台注入的数据根（移动端在入口写入应用私有目录）。
/// Android 上 dirs::data_dir() 返回 None，会退化成相对路径导致配置/收藏无法落盘。
static DATA_ROOT: OnceLock<PathBuf> = OnceLock::new();

/// 入口注入平台数据根（仅首次生效）。
pub fn set_data_root(p: PathBuf) {
    let _ = DATA_ROOT.set(p);
}

/// 应用数据根目录（跨平台）。
/// 优先级：`ZPLAYER_DATA_DIR` 环境变量 → 平台注入目录 → dirs crate。
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ZPLAYER_DATA_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    let base = DATA_ROOT
        .get()
        .cloned()
        .or_else(dirs::data_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("ZPlayerMusic")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("log")
}

/// 本地资源扫描目录：
/// - 配置了 `local.dir` 时用自定义目录；
/// - 否则默认扫描**二进制所在目录下的 media**（便携式布局，解压即用）；
/// - 安装目录不可写（如 Program Files）时退回 `data_dir/media`，避免功能不可用。
/// 目录不存在时自动创建，确保"放入文件即扫描"的体验。
pub fn local_dir(cfg: &AppConfig) -> PathBuf {
    let custom = cfg.local.dir.trim();
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        let _ = std::fs::create_dir_all(&p);
        return p;
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(data_dir);
    let dir = exe_dir.join("media");
    if std::fs::create_dir_all(&dir).is_ok() {
        dir
    } else {
        let fallback = data_dir().join("media");
        let _ = std::fs::create_dir_all(&fallback);
        fallback
    }
}

/// 缓存根目录：优先用户统一设置的 cache.dir，其次默认 data_dir/cache。
pub fn cache_root(cfg: &AppConfig) -> PathBuf {
    let u = cfg.cache.dir.trim();
    if !u.is_empty() {
        PathBuf::from(u)
    } else {
        cache_dir()
    }
}

/// 封面缓存目录：统一根目录/covers；若配置了旧式 cover-dir 则沿用；空用默认。
pub fn cover_cache_dir(cfg: &AppConfig) -> PathBuf {
    if !cfg.cache.dir.trim().is_empty() {
        cache_root(cfg).join("covers")
    } else if !cfg.cache.cover_dir.trim().is_empty() {
        PathBuf::from(cfg.cache.cover_dir.trim())
    } else {
        cache_dir().join("covers")
    }
}

/// 歌曲缓存目录：统一根目录/songs；若配置了旧式 song-dir 则沿用；空用默认。
pub fn song_cache_dir(cfg: &AppConfig) -> PathBuf {
    if !cfg.cache.dir.trim().is_empty() {
        cache_root(cfg).join("songs")
    } else if !cfg.cache.song_dir.trim().is_empty() {
        PathBuf::from(cfg.cache.song_dir.trim())
    } else {
        cache_dir().join("songs")
    }
}

/// 歌词缓存目录：统一根目录/lyrics；空用默认。
pub fn lyric_cache_dir(cfg: &AppConfig) -> PathBuf {
    if !cfg.cache.dir.trim().is_empty() {
        cache_root(cfg).join("lyrics")
    } else {
        cache_dir().join("lyrics")
    }
}

// ---- 配置结构 ----

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AppConfig {
    #[serde(rename = "app")]
    pub app: AppSection,
    #[serde(rename = "play")]
    pub play: PlaySection,
    #[serde(rename = "ui")]
    pub ui: UiSection,
    #[serde(rename = "network")]
    pub network: NetworkSection,
    #[serde(rename = "lx")]
    pub lx: LxSection,
    #[serde(rename = "cache")]
    pub cache: CacheSection,
    #[serde(rename = "local")]
    pub local: LocalSection,
    #[serde(rename = "shortcut")]
    pub shortcut: ShortcutSection,
    #[serde(rename = "last-song")]
    pub last_song: LastSongSection,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app: AppSection::default(),
            play: PlaySection::default(),
            ui: UiSection::default(),
            network: NetworkSection::default(),
            lx: LxSection::default(),
            cache: CacheSection::default(),
            local: LocalSection::default(),
            shortcut: ShortcutSection::default(),
            last_song: LastSongSection::default(),
        }
    }
}

/// 应用内快捷键（窗口聚焦时生效）。
/// 组合字符串格式固定为「修饰键 Ctrl/Alt/Shift 顺序 + 键名」，如 "Ctrl+Left"、"Space"、"Ctrl+,"。
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct ShortcutSection {
    pub play_pause: String,
    pub prev: String,
    pub next: String,
    pub seek_back: String,
    pub seek_forward: String,
    pub volume_up: String,
    pub volume_down: String,
    pub mute: String,
    pub home: String,
    pub settings: String,
    pub hide: String,
}

impl Default for ShortcutSection {
    fn default() -> Self {
        Self {
            play_pause: "Space".to_string(),
            prev: "Ctrl+Left".to_string(),
            next: "Ctrl+Right".to_string(),
            seek_back: "Ctrl+Shift+Left".to_string(),
            seek_forward: "Ctrl+Shift+Right".to_string(),
            volume_up: "Ctrl+Up".to_string(),
            volume_down: "Ctrl+Down".to_string(),
            mute: "Ctrl+M".to_string(),
            home: "Ctrl+Home".to_string(),
            settings: "Ctrl+,".to_string(),
            hide: "Ctrl+H".to_string(),
        }
    }
}

/// 快捷键动作 id（顺序与设置页列表、UI 模型一一对应）
pub const SHORTCUT_IDS: [&str; 11] = [
    "play_pause", "prev", "next", "seek_back", "seek_forward",
    "volume_up", "volume_down", "mute", "home", "settings", "hide",
];

impl ShortcutSection {
    pub fn get(&self, id: &str) -> &str {
        match id {
            "play_pause" => &self.play_pause,
            "prev" => &self.prev,
            "next" => &self.next,
            "seek_back" => &self.seek_back,
            "seek_forward" => &self.seek_forward,
            "volume_up" => &self.volume_up,
            "volume_down" => &self.volume_down,
            "mute" => &self.mute,
            "home" => &self.home,
            "settings" => &self.settings,
            "hide" => &self.hide,
            _ => "",
        }
    }

    pub fn set(&mut self, id: &str, combo: &str) {
        let slot = match id {
            "play_pause" => &mut self.play_pause,
            "prev" => &mut self.prev,
            "next" => &mut self.next,
            "seek_back" => &mut self.seek_back,
            "seek_forward" => &mut self.seek_forward,
            "volume_up" => &mut self.volume_up,
            "volume_down" => &mut self.volume_down,
            "mute" => &mut self.mute,
            "home" => &mut self.home,
            "settings" => &mut self.settings,
            "hide" => &mut self.hide,
            _ => return,
        };
        *slot = combo.to_string();
    }

    /// 按 SHORTCUT_IDS 顺序导出组合键列表（UI 模型）
    pub fn to_list(&self) -> Vec<String> {
        SHORTCUT_IDS.iter().map(|id| self.get(id).to_string()).collect()
    }

    /// 该组合键是否已被其他动作占用（返回占用的动作 id）
    pub fn conflicting(&self, combo: &str, exclude_id: &str) -> Option<&'static str> {
        SHORTCUT_IDS
            .iter()
            .find(|id| **id != exclude_id && self.get(id).eq_ignore_ascii_case(combo))
            .copied()
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct AppSection {
    /// 深浅主题
    pub dark: bool,
    /// 窗口位置（None = 首次启动居中）
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    /// 音量 0..1（退出时统一写盘）
    pub volume: f32,
    /// 缓存容量上限 MB（0 = 不限制；最小建议 1024）
    pub cache_limit_mb: u64,
    /// 开机自启
    pub auto_launch: bool,
    /// 语言短码（"zh"/"en" 或用户扩展如 "fr"；空 = 简体中文）
    pub language_code: String,
    /// 旧版语言索引（0=简体 1=English 2=繁體），仅启动迁移用，新版不写入
    pub language_index: Option<i32>,
    /// 字体设置（0=默认 1=微软雅黑 2=思源黑体）
    pub font_index: i32,
}

impl Default for AppSection {
    fn default() -> Self {
        Self {
            dark: false,
            window_x: None,
            window_y: None,
            volume: 0.8,
            cache_limit_mb: 0,
            auto_launch: false,
            language_code: "zh".to_string(),
            language_index: None,
            font_index: 0,
        }
    }
}

/// 播放设置
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct PlaySection {
    /// 音质（0=标准 1=高音质 2=无损）
    pub quality_index: i32,
    /// 音源（0=网易云 1=QQ 2=酷狗 3=酷我 4=咪咕）
    pub source_index: i32,
    /// 自动播放
    pub auto_play: bool,
    /// 音频输出设备（0=默认 1=扬声器 2=耳机）
    pub output_index: i32,
    /// 当前输出设备名（"默认设备" 或具体设备名；持久化用）
    pub output_device: String,
    /// MV 清晰度（0=480p 1=720p 2=1080p）：跨视频、跨进程记忆
    pub mv_quality_index: i32,
}

impl Default for PlaySection {
    fn default() -> Self {
        Self {
            quality_index: 0,
            source_index: 0,
            auto_play: false,
            output_index: 0,
            output_device: "默认设备".to_string(),
            mv_quality_index: 2,
        }
    }
}

/// 界面设置
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct UiSection {
    /// 主题色（0=紫青渐变 1=青绿渐变 2=橙红渐变 3=蓝紫渐变）
    pub theme_color_index: i32,
    /// 歌词布局（true=上封面/下歌词, false=左封面/右歌词）
    pub lyrics_style: bool,
    /// 关闭行为（0=每次询问 1=最小化到托盘 2=直接退出）
    pub close_behavior_index: i32,
    /// 桌面歌词悬浮窗开关（仅桌面端）
    pub desktop_lyrics: bool,
    /// 桌面歌词锁定（= 鼠标点击穿透，仅桌面端）
    pub desktop_lyrics_locked: bool,
    /// 桌面歌词字号（px）
    pub desktop_lyrics_font_size: i32,
    /// 桌面歌词字体风格索引（0=微软雅黑 1=宋体 2=楷体 3=黑体）
    pub desktop_lyrics_font_index: i32,
    /// 桌面歌词已唱配色预设索引（见 ui/desktop/components/lyrics-ink.slint）
    pub desktop_lyrics_sung_index: i32,
    /// 桌面歌词未唱配色预设索引（见 ui/desktop/components/lyrics-ink.slint）
    pub desktop_lyrics_unsung_index: i32,
    /// 桌面歌词字体黑色描边（增强复杂背景下的可读性）
    pub desktop_lyrics_outline: bool,
}

impl Default for UiSection {
    fn default() -> Self {
        Self {
            theme_color_index: 0,
            lyrics_style: true,
            close_behavior_index: 0,
            desktop_lyrics: false,
            desktop_lyrics_locked: false,
            desktop_lyrics_font_size: 24,
            desktop_lyrics_font_index: 0,
            desktop_lyrics_sung_index: 0,
            desktop_lyrics_unsung_index: 0,
            desktop_lyrics_outline: true,
        }
    }
}

/// 洛雪（LX Music）音源脚本设置
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct LxSection {
    /// 洛雪兜底开关（网易云直链失败后是否尝试音源脚本）
    pub fallback_enabled: bool,
    /// 启用的音源脚本文件名列表（兜底按此顺序逐源解析）
    pub enabled: Vec<String>,
    /// 自动更新社区音源开关
    pub auto_update: bool,
    /// 内置维护清单更新源标识（预留；空 = 使用默认内置清单）
    pub update_source: String,
    /// 是否已写入内置默认脚本（防止用户清空启用列表后被重复拉默认）
    pub seeded: bool,
}

impl Default for LxSection {
    fn default() -> Self {
        Self {
            fallback_enabled: true,
            enabled: Vec::new(),
            auto_update: false,
            update_source: String::new(),
            seeded: false,
        }
    }
}

/// 网络设置
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct NetworkSection {
    /// 下载线程数（0=1 1=2 2=4 3=8 4=16）
    pub threads_index: i32,
    /// 网络缓冲时长（0=自动 1=3s 2=5s 3=10s）
    pub buffer_index: i32,
    /// 代理开关
    pub proxy_enabled: bool,
    /// 代理类型（0=HTTP 1=HTTPS 2=SOCKS4 3=SOCKS5）
    pub proxy_type_index: i32,
    /// 代理地址（host）
    pub proxy_host: String,
    /// 代理端口
    pub proxy_port: u16,
    /// realip 开关
    pub realip_enabled: bool,
    /// realip 手动地址
    pub realip_addr: String,
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            threads_index: 2,
            buffer_index: 0,
            proxy_enabled: false,
            proxy_type_index: 0,
            proxy_host: String::new(),
            proxy_port: 0,
            realip_enabled: false,
            realip_addr: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct CacheSection {
    /// 封面缓存目录（空 = 默认 data_dir/cache/covers）
    pub cover_dir: String,
    /// 歌曲缓存目录（空 = 默认 data_dir/cache/songs）
    pub song_dir: String,
    /// 磁盘缓存开关
    pub enabled: bool,
    /// 统一缓存目录（封面+歌曲同配置；空 = 默认，非空时覆盖 cover_dir/song_dir）
    pub dir: String,
    /// 清理策略（0=最近少用 LRU 1=先进先出 FIFO）
    pub evict_strategy: i32,
}

impl Default for CacheSection {
    fn default() -> Self {
        Self {
            cover_dir: String::new(),
            song_dir: String::new(),
            enabled: true,
            dir: String::new(),
            evict_strategy: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct LocalSection {
    /// 本地资源扫描目录（空 = 二进制所在目录下的 media）
    pub dir: String,
}

impl Default for LocalSection {
    fn default() -> Self {
        Self { dir: String::new() }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct LastSongSection {
    /// 上次播放歌曲 id（i64，网易云歌曲 id 可能超 i32）
    pub id: i64,
    pub name: String,
    pub artist: String,
    pub album: String,
    /// 总时长（秒）
    pub duration_sec: f32,
    /// 上次播放位置（秒）
    pub progress_sec: f32,
    /// 封面来源 URL（供启动时从磁盘封面缓存恢复封面图）
    pub cover_url: String,
}

impl Default for LastSongSection {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            artist: String::new(),
            album: String::new(),
            duration_sec: 0.0,
            progress_sec: 0.0,
            cover_url: String::new(),
        }
    }
}

impl AppConfig {
    /// 字体索引 → 全局 font-family（appwindow 的 default-font-family 消费）。
    /// 0=默认(Segoe UI) 1=微软雅黑 2=思源黑体
    pub fn font_family(idx: i32) -> &'static str {
        match idx {
            1 => "Microsoft YaHei",
            2 => "Source Han Sans SC",
            _ => "Segoe UI",
        }
    }

    /// 加载配置；文件缺失 / 解析失败时回退默认值。
    pub fn load() -> Self {
        let mut c = match crate::platform::storage::read_text("config.toml") {
            Some(s) => toml::from_str(&s).unwrap_or_default(),
            None => Self::default(),
        };
        // 迁移旧版语言索引（0=简体 1=English 2=繁體）→ 短码；迁移后清除索引字段
        if let Some(i) = c.app.language_index {
            if c.app.language_code.is_empty() {
                c.app.language_code = match i {
                    1 => "en".to_string(),
                    _ => "zh".to_string(),
                };
            }
            c.app.language_index = None;
        }
        c
    }

    /// 保存配置（经 storage 门面，桌面为原子写：先写临时文件再改名）。
    pub fn save(&self) {
        if let Ok(s) = toml::to_string_pretty(self) {
            let _ = crate::platform::storage::write_text("config.toml", &s);
        }
    }
}
