# ZPlayer 跨平台架构：三维 feature 平台方案

> 状态：定案（2026-10-21）
> 目标：Slint + Rust 播放器全面跨平台 —— 桌面（Win/macOS/Linux）+ Web（WASM 浏览器）+ 移动原生（Android/iOS）。
> 核心原则：
>
> **平台差异收敛进一个薄的&#x20;**
>
> `platform`
>
> **&#x20;门面；业务（core/network/load/handlers）永远是平台无关的纯 Rust，只依赖门面 trait，永不碰平台实现。**



***

## 1. 三维平台定义



| 维度            | 判定                                          | 目标                       |
| ------------- | ------------------------------------------- | ------------------------ |
| `desktop`（默认） | `not(wasm32) and not(android) and not(ios)` | Windows · macOS · Linux  |
| `web`         | `target_arch = wasm32`                      | 浏览器（wasm 只能 = 浏览器）       |
| `mobile`      | `target_os = android \| ios`                | 原生移动（arm/aarch64，非 wasm） |

> **关键**
>
> ：
>
> `wasm32`
>
>  只精确等于 "web"，不能当 "非桌面" 的分界；移动是原生，用 
>
> `target_os`
>
>  判定。

## 2. 能力 × 平台矩阵



| 能力           | desktop                     | web                     | mobile                       |
| ------------ | --------------------------- | ----------------------- | ---------------------------- |
| 音频播放         | rodio/cpal **原生解码 bytes**   | WebAudio/HTMLAudio（URL） | 系统媒体 ExoPlayer/AVPlayer（URL） |
| 存储           | 文件系统（dirs + fs）             | IndexedDB               | 沙盒文件系统                       |
| 窗口控制         | 标题栏 / 迷你 / 最大化 / 拖拽         | 无                       | 无（有系统导航 + 状态栏 safe-area）     |
| 开机自启         | 有（注册表 /launchctl/autostart） | 无                       | 无                            |
| 文件 / 文件夹选择   | rfd                         | `<input type=file>`     | 系统选择器                        |
| 日志           | simplelog 文件                | console                 | 系统日志                         |
| 洛雪脚本 QuickJS | rquickjs（线程）                | rquickjs-wasm（**需验证**）  | rquickjs                     |

**共用、无条件依赖**（所有平台都有的核心）：slint、serde/serde\_json、reqwest (async)、weapi 加密套件（aes/cbc/base64/hex/rand/num-bigint）、image（封面）、lofty（改内存解析）、网易云 + 全部音源模块（gdmusic/migu/kuwo/kugou/qq/joox）。这些**不进任何 feature**，永远编译。

## 3. Cargo.toml 装配



```
[features]
default = ["desktop"]
desktop = ["dep:rodio", "dep:cpal", "dep:rfd", "dep:winapi",
           "dep:simplelog", "unm-ytdl"]        # 桌面能力集
web     = ["dep:wasm-bindgen", "dep:web-sys", "dep:js-sys"]
mobile  = ["dep:media", "dep:dirs"]             # 系统媒体后端

[target.'cfg(target_arch="wasm32")'.dependencies]
wasm-bindgen = "0.2"
```



* 平台专属依赖挂在对应 feature 下（当前 Cargo.toml 是 "全部无条件"—— 这正是 wasm 编不过的直接原因之一）。

* 仅由桌面使用的依赖：rodio /cpal/rfd /winapi/simplelog 等标 `optional` 并挂 `desktop`。

* `dirs`：desktop + mobile 都用（路径 / 沙盒），视需要保持共用或挂两者。

## 4. 平台门面（src/platform/）—— 唯一藏平台差异的地方

每个能力 = "trait + 各平台实现 + 按 feature 选暴露一个"：



```
// platform/audio/mod.rs
trait AudioBackend { play_url; play_bytes; pause; seek; stop; volume; status; list_devices; }

#[cfg(feature = "desktop")]  mod desktop;  pub use desktop::DesktopAudio as Audio;
#[cfg(feature = "web")]      mod web;      pub use web::WebAudio        as Audio;
#[cfg(feature = "mobile")]   mod mobile;   pub use mobile::MobileAudio  as Audio;
```

建议目录：



```
src/platform/
  mod.rs            # trait 汇总 + cfg/feature 装配
  audio/{mod,desktop,web,mobile}.rs
  storage/{mod,fs,indexeddb}.rs
  window/{mod,desktop}.rs      # 仅桌面
  sys/{mod,desktop}.rs         # 自启动等（仅桌面）
  dialog.rs                    # 文件/文件夹选择
```

## 4.1 存储边界（Storage trait 覆盖范围，2026-10-21 定案）

`Storage` trait 只覆盖**纯键值数据**：`config`（config.toml）、`liked`（liked.json）。

- **cache（封面/歌曲/歌词目录级缓存 + 容量治理：mtime LRU / 总占用统计 / 递归清理）不走 Storage trait**——目录级缓存 + 容量治理是文件系统能力，IndexedDB（web）无 mtime / 总占用原语。cache 走平台分支：桌面保留 fs 直写，web 用浏览器缓存（Storage Quota）或禁用容量清理。
- **logger（simplelog 日志文件）不走 Storage trait**——日志非"数据存储"。走平台分支：桌面 simplelog 文件，web console。
- **local.rs 本地音乐扫描**属桌面文件系统能力，不进 Storage（web/mobile 改为文件选择上传）。

## 5. 业务层只依赖门面（逻辑零平台代码）



```
// core/player.rs —— 平台无关
pub fn play(url: &str) {
    platform::audio::Audio::play_url(url);   // 底下是 rodio / WebAudio / ExoPlayer 无所谓
}
```

`core / network / load / handlers` 全不碰 `#[cfg]`，纯 Rust。这是后端能三端复用而不重写的原因。

## 6. UI 分层（两个骨架目录，不是三个）



```
ui/
  components/   # 共用组件库：歌词/进度条/列表/卡片/toast/主题皮肤/图标/loading
  desktop/      # 桌面 + web 共用骨架
  mobile/       # 移动独立骨架（底部 Tab + 全屏播放页 + 触摸交互）
```



* **desktop 与 web 共用一套骨架**：差异是 "外壳" 级。**web 构建仍编译标题栏**（品牌 / 返回 / 搜索等应用内功能保留），仅 **窗口控制组（最小化 / 最大化 / 关闭 / 迷你）整体隐藏**——用响应式布局（`HorizontalLayout` + `visible: false`）让剩余按钮自动顺位补位，绝不固定留空。

* **浏览器无窗口控制 API**：JS 无法最小化 / 最大化系统窗口，`window.close()` 仅对脚本打开的窗口有效、普通标签被浏览器忽略 → web **不做 "与浏览器关联"** 窗口功能，窗口 / 标签由浏览器自身管理（用户用浏览器标签栏关闭）。`requestFullscreen` 是页面级全屏、语义不同于 "窗口最大化"，默认不做。

* **mobile 独立骨架**：交互范式不同（底部 Tab / 全屏播放页 / 触摸），无法靠条件隐藏硬合。

* 条件判断分层：


  * **能不能有（能力存在性）→ 编译期 feature/cfg**（如窗口控制组在 web 构建整体隐藏 / 不编译其逻辑）。

  * **要不要显示（能力内自适应）→ 运行时判断**（touch / 视口宽度，用于 web 在桌面 / 移动浏览器自适应）。

* Web 在移动浏览器：先走 "web = 桌面骨架"；窄视口自适应留到移动端上线后再看。

## 7. 落地顺序



1. **Cargo.toml 三维 feature 拆分**（定义 feature + 桌面依赖挂 desktop，default 不回归）；

2. **platform 门面（audio/storage）**：trait + 现有桌面实现抽成 desktop 实现，业务改调 trait（逻辑不动）；

3. **代码平台门控**：依赖与代码 cfg 同步，让 web/mobile 可编译；

4. 补 web /mobile 实现；

5. UI 按两个骨架目录推进。

## 7. 音源（LX / UNM）平台门控（2026-10-21 已落地）

web（wasm32）下 `rquickjs` 与 `unm_engine` 无法编译（§8 实测）→ LX/UNM 仅在 desktop/mobile 启用，web 由主源兜底：

- Cargo.toml：`rquickjs` + `unm_engine` 系列移入 `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`（web 不拉）；
- `sources/mod.rs`：`pub mod lx / lx_scripts / unm` 加 `#[cfg(not(target_arch = "wasm32"))]`；`resolve()` 的 unm 分支与 sources 数组按 cfg 追加；依赖 rquickjs/unm 的测试同条件；
- `core/fetch.rs`：洛雪脚本兜底段 `#[cfg(not(target_arch = "wasm32"))]`；
- gdmusic / 内置 7 源为纯 HTTP（reqwest），web 可编译，保留。
- **已落地（含 UI 侧运行时门控）**：Rust 侧（settings.rs on_settings_clicked 41/44/45、main.rs 脚本列表注入 + seeding 块）均 `#[cfg(not(target_arch = "wasm32"))]`；**Slint UI 侧**走运行时 is-web property（AppWindow in-out → TitleBar / SettingsPage → PlayTab）：web 下隐藏标题栏窗口控制组（迷你/最小化/关闭，HorizontalLayout isible:false 顺位补位）与设置页音源脚本区。Rust 端 `#[cfg(target_arch = "wasm32")] ui.set_is_web(true)` 注入。desktop 编译验证 exit=0。


## 7.2 mobile 落地路径（2026-10-21 规划，A 路线优先项）

**现状**：storage 门面（FsStorage + `data_dir()`）已跨桌面/mobile 通用（`dirs` 在 Android/iOS 返回 app 沙盒目录，无需单独 mobile 分支）。audio 门面待 `audio/mobile.rs`。web 后期适配。

**关键约束**：mobile 后端的系统媒体播放 / mobile UI 骨架 **全部需 Android/iOS 工具链才能编译验证**。本机为 Windows，**只能验证 Android 侧（NDK/JDK）；iOS 需 macOS/CI**。不装工具链时写 mobile 代码属"盲写"，风险高。

**分块（按序）**：
1. **编译验证路径（前置）**：Android `rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android` + Android NDK/JDK + Slint Android 渲染后端；iOS `aarch64-apple-ios` + Xcode + Slint iOS 后端（仅 macOS）。
2. **audio/mobile.rs**：系统媒体（Android MediaPlayer via JNI / iOS AVFoundation via FFI），实现 audio 门面 14 个函数（play_at/stop/toggle/status/…），WebAudio 留给 web。
3. **UI mobile 骨架**：底部 Tab + 全屏播放页（独立骨架，不等同桌面）。
4. **Cargo.toml mobile feature 装配**：挂 Slint 移动端渲染 + 系统媒体依赖。

**验证**：真机 / 模拟器 / CI（cargo build --target aarch64-linux-android 等）。建议 Windows 先装 Android 工具链验证 Android 侧，iOS 留 macOS/CI。

**风险**：Slint 移动端渲染（Fluent 风格在 Android/iOS 的原生质感）、系统媒体播放字节流 vs URL 语义差异、沙盒存储配额。
## 8. 已知风险 / 待验证



* **rquickjs（QuickJS）编 wasm**（2026-10-21 实测）：**wasm 默认不可用**——`rquickjs-sys` 不为 `wasm32-unknown-unknown` 预编译绑定，需 `bindgen` feature + **clang/emscripten 工具链**编译 QuickJS C 源码（当前环境无 clang）。→ wasm 版洛雪脚本**退化为不启用**（除非投入工具链）。**mobile 原生不受影响**（QuickJS 原生可编）。

* **unm\_engine 编 wasm**（2026-10-21 实测）：**编译不过**——`unm_request` 未适配 wasm（reqwest 的 wasm 后端无 `timeout`/`proxy` 方法，且 `js_sys::futures::JsFuture` 非 Send 导致 async 约束失败）。→ web 版 UNM 兜底**不可用**（除非 fork 适配 `unm_request`）。**mobile 原生不受影响**（reqwest native 后端正常）。

* **unm\_engine\_ytdl**：依赖 winapi + 外部 yt-dlp 进程，wasm 必挂，需 feature 裁剪（仅留纯 HTTP 引擎 migu/kugou/kuwo/pyncm/qq）。

* **unm\_request**：reqwest 需 rustls + wasm（去掉 native-tls）。实测 wasm 下 `unm_request` 自身未适配（timeout/proxy 方法缺失、JsFuture 非 Send），web 不可用；mobile 原生正常。

* **播放器换 WebAudio / 系统媒体**：最大硬障碍，播放管线重写（播 URL 而非字节）。

* **lofty**：从内存字节解析可 wasm；但 "本地目录扫描" 功能是桌面专属（web/mobile 改为文件选择上传）。

* **LGPL-3.0**：unm\_engine 系列为 LGPL-3.0，静态链接需考虑合规（提供可替换 relink 能力）。

## 9. Web 主通道定案（A 方案）+ 阶段 0 / 1a 落地（2026-10-22）

### 9.1 B-wasm（WASI 当后端）被否决
- reqwest 编 `wasm32-wasip1` 编译成功但走 web-sys 浏览器后端，纯 WASI runtime 跑不了 HTTP。
- 改投 wasmtime 50 + wstd 0.6.8（`wasm32-wasip2`）POC：HTTP client 与 `#[wstd::http_server]` 组件均编译成功，但 `wasmtime run` 缺 `wasi:http/types@0.2.12` host、`wasmtime serve` 缺 `wasi:cli/environment@0.2.12` 匹配 → **运行层受阻**。
- **决定性否决**：目标要上传 **GitHub Pages / Cloudflare Pages 纯静态托管**，不提供 WASI runtime 进程 → B-wasm 无法部署。

### 9.2 A 方案定案：浏览器 wasm + gdmusic
- `wasm32-unknown-unknown` + **gdmusic 聚合 API** 作解析主通道（实测 `access-control-allow-origin` 回显任意 Origin + 全 methods → 浏览器 fetch 可用）；网易云原生接口无 CORS 头，浏览器不可直连。
- 搜索 `?types=search&source=<joox|tidal|netease>&name=<q>`；可播 URL `?types=url&source=<src>&id=<id>&br=999`（直接返回可播网易云 flac/mp3）。封面/歌词待 1b 补测。
- HTMLAudio/WebAudio 播放外链（无需 CORS）。上传 GH/CF Pages 静态托管即运行，**不部署后端 / WASI runtime**。

### 9.3 依赖替换（本轮落地，Cargo.toml）
- **rfd → tinyfiledialogs 3.9**：rfd 无条件依赖 web-sys 且需 `UserActivation` feature，与 web 端 Cargo.lock 冲突。API：`select_folder_dialog(title, path)`、`open_file_dialog(title, path, Option<(&[&str], &str)>)`。
- **miniaudio 弃、保留 rodio/cpal（0.19 / 0.15）**：miniaudio 依赖 `ep-miniaudio-sys→bindgen 0.54→clang-sys 0.29`，与 slint skia 渲染器（bindgen 0.72→clang-sys 1.x）在 `links=clang` 冲突，且本机无 clang → 编译不了。rodio/cpal 无 bindgen/clang。
- **UNM/LX（rquickjs + unm_engine 系列）移入 desktop feature（optional）**：不再放 `[target.not-wasm32]` 段。Cargo 会解析所有 target+feature 组合到统一 lock，target 段无法隔离；`unm_engine_kugou→reqwest ^0.11.14→web-sys/ReadableStream` 与 web 端冲突，只能靠 desktop feature 隔离（web 构建不启用 desktop 就不解析它们）。
- **web 端去 reqwest**：用 **web-sys fetch** 直调 gdmusic，绕开 reqwest 的 `web-sys/ReadableStream` 依赖；web 无 tokio。
- **web-sys 钉 `=0.3.106`**（精确依赖 wasm-bindgen `=0.2.129`）；feature 名必须用类型名（`Window`/`Document`/`HtmlAudioElement`/`AudioContext`/`Request`/`Response`/`ResponseInit`/`Headers`/`console` 等）——**无小写 `window`/`document`、无 `Audio`**（无效 feature 名会报 "does not have that feature"）。

### 9.4 代码平台门控（阶段 0 → 1a）
- `main.rs`：`core`（fetch/logger 依赖 reqwest/simplelog）整体 `#[cfg(not(target_arch="wasm32"))]`；`#![cfg_attr(target_arch="wasm32", no_main)]`；桌面 persist/network/load/handlers + 桌面 main + 图标均 `not(wasm32)`；`build.rs` 用 `CARGO_CFG_TARGET_ARCH != "wasm32"` 门控 winres。
- `src/platform/audio/web.rs`：`play_url(url, total_sec)`（HTMLAudio 播外链无 CORS）+ 完整门面契约；web_main 自包含（slint! 内联 UI：LineEdit/Button import std-widgets、`for s[idx] in songs : Element { }` 语法 + gdmusic web-sys fetch + `crate::platform::audio::play_url`）。
- **结果（2026-10-22）**：`cargo build`（desktop 默认）**exit=0** + `cargo build --no-default-features --features web --target wasm32-unknown-unknown` **exit=0**。

### 9.5 待办
- web 部署产物：wasm-bindgen-cli（须 =0.2.129，与 web-sys 0.3.106 配套）生成 `pkg`（js glue + wasm）+ `index.html`，上传静态托管即可运行。
- 阶段 1b：core 的 web 移植（fetch/logger 用 web-sys）、完整 UI/handlers 移植、歌词/封面链路补测、存储 WebStorage 接线。
- gdmusic 为第三方公共服务，需在 1a 验证限流/稳定性边界；把 old 内置源迁移为项目内置默认。