# ZPlayer Music — Rust 模块化重构计划

> 状态：**已完成**（load/home + main.rs 函数拆分 + handlers 回调拆分 + 平铺模块按职责归入四目录，编译全绿）
> 决策人：用户（组织=`src/load/` 入口 + `src/load/home/` 子目录；回调=按业务拆多文件；平铺模块=`core/persist/network/platform` 四目录）

## 目标
消除 `load.rs`(453行) 与 `main.rs`(1810行，其中 `fn main` ~1220行) 的单文件堆积，
按职责拆分，`main.rs` 只保留 `mod` 声明 + `fn main` 启动编排。

---

## 一、最终目录结构（已完成）

```
src/
├── main.rs              只留 mod 声明(re-export) + fn main 启动编排
├── load/                业务加载（入口 re-export，保持 crate::load::X 路径）
│   └── home/            首页板块：load_initial / covers / playlists / artists / albums / newsongs / toptracks
├── handlers/            UI 回调（mod.rs 聚合 Ctx/ToastCtx + settings/cache/search/player/modwin 各带 setup）
├── core/                领域逻辑与基础设施：song_util / lyrics / fetch / queue / logger
├── persist/             持久化与存储：config / state / liked / cache
├── network/             网络层：netease/ sources/ service / proxy（原 net.rs）
└── platform/            平台集成：winshell / autolaunch / audio（原 player.rs）
```

平铺模块按职责归入二级子目录，各目录 `mod.rs` 负责 `pub mod` 声明；
`main.rs` 顶层用 `pub use core::...` 等 re-export 保持 `crate::song_util` 等旧路径，
并 `pub use network::proxy as net;` / `pub use platform::audio as player;` 兼容 `crate::net`/`crate::player`。

## 二、main.rs 拆分 → 按业务多文件（每个带自己的 setup 入口）

```
src/liked.rs          喜欢列表：liked_path / load_liked / save_liked / is_liked / sync_liked
src/lyrics.rs         parse_lrc（歌词解析，原 main.rs:70-97）
src/winshell.rs       mod dwm（圆角）+ mod winmin（最小尺寸）+ set_mini_drawer（原 141-376,377-401）
src/queue.rs          播放队列：pick_queue_target / play_queue_index / record_last_song（原 463-537）
src/state.rs          save_state 落盘 + refresh_cache_stats（原 423-462）
src/fetch.rs          fetch_song_bytes 歌曲字节下载（原 98-140）
src/handlers/         回调接线（按业务，每个文件一个 pub setup 入口）
  └── settings.rs     on_settings_* 全部（toggle/select/input/clicked/proxy/clear-cache/check-update/donate/navigate）
  └── player.rs       统一解析器 make_resolver + on_play_queue / 播放/暂停/seek/进度 / 收藏/上一首/下一首/播放模式
  └── modwin.rs       窗口控制/迷你模式/标题栏拖拽/splash 动画/跑马灯（原 queue 一档由 modwin 承载窗口类回调）
  └── search.rs       on_search / 建议项 / 歌单分类触底分页轮询
  └── cache.rs        缓存相关 on_*（缓存目录/清理/刷新统计/容量上限循环）
  └── mod.rs          定义 ToastCtx + Ctx{app, ui, shared, toast, rt, client}；setup 由 main 依次调用各业务 setup
```
`fn main` 瘦身为：初始化(日志/config/net/player/cache) → 启动回填(各 set_*) → 装配 handlers →
`run()`. 回调上下文（`ui`/`shared: Rc<RefCell<AppConfig>>`/`toast_ctx`）通过 setup 参数传递。

## 三、执行要点
- 纯**逻辑搬迁**：不改行为、不改 Slint 接口、不动用户 detail/首页重构中的代码。
- 每拆一个文件跑一次 `cargo check`（若 detail/home 仍阻塞，先以结构核对为准，接口同步后统一验证）。
- 拆完后：`cargo check` 全绿 + 手动跑一遍设置页各 tab（图标/弹窗/代理/realip/音频设备/开机自启/缓存）回归。
- settings 回调搬迁时注意：proxy-type、output-devices 等本轮新加的在途接线一并纳入 settings handler，勿漏。

## 四、与 detail/首页重构的关系
appwindow.slint 当前引用 `pages/detail.slint`、`HomePage.recommend-playlists/hero-count/open-detail`、`DetailPage` 未同步，阻塞整包编译。
本重构只动 `src/*.rs`，与它正交；拆完后若仍阻塞，由用户先同步 detail/首页接口，再做最终验证。
