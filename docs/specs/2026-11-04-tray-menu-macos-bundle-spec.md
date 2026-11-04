# 托盘自绘右键菜单 + macOS .app 打包 — 技术规格

日期：2026-11-04 ｜ 状态：待审批

## 1. 背景与目标

### 1.1 托盘自绘弹窗（Windows/macOS/Linux）
现状：`src/platform/tray.rs` 使用 `tray-icon` 0.26 原生菜单（显示/上一首/播放暂停/下一首/退出），样式不可定制。
目标：按参考图实现 Slint 自绘弹窗 —— 白色（随主题）圆角菜单、左侧竖线装饰、hover 灰底高亮，菜单结构：

```
┌──────────────────┐
│ 当前歌曲名（粗体标题）│
│ 显示              │
│ 收藏 / 取消收藏     │
│ ──────────────── │
│ 上一首            │
│ 播放 / 暂停        │
│ 下一首            │
│ ──────────────── │
│ 语言            › │  ← 二级子菜单（语言列表，当前语言打勾）
│ ──────────────── │
│ 退出              │
└──────────────────┘
```

### 1.2 macOS 三问题
现状：CI 直接发布裸 Mach-O 二进制，导致：
- (a) 运行前 icon 显示通用 exec 图标
- (b) 双击启动附带终端命令窗口
- (c) 状态栏托盘不显示（根因：`tray.rs` 被 `#[cfg(windows)]` 门控，macOS 从未编译托盘代码）

三者统一通过 GitHub CI 打包 `ZPlayer Music.app` bundle 解决（a/b 靠 bundle + Info.plist；c 靠放宽 cfg + bundle 提供正常运行环境）。

## 2. 关键调研结论（已核实源码）

| 事实 | 出处 |
|---|---|
| `TrayIconEvent::Click` 携带物理像素 `position` 与托盘图标 `rect`，可用于弹窗定位 | tray-icon 0.26 事件结构，tray.rs 现有接收线程 |
| slint 1.18.1 `unstable-winit-030` 无 `on_winit_window_event` API，无法钩子窗口失焦事件 | slint-1.18.1/lib.rs `winit_030` 模块 |
| main.rs 的 `with_winit_window_attributes_hook` 对**所有**窗口生效（decorations(false)/visible(false)/min 900x560/记住位置），托盘窗口必须豁免 | src/main.rs:121-132 |
| hook 在 `create_window_adapter()`（即 `TrayMenu::new()`）内同步触发，属性快照存于 `WinitWindowOrNone::None(attributes)`，实际 winit 窗口延迟到 show 时才按快照创建 | i-slint-backend-winit-1.18.1 lib.rs:894-903 / winitwindowadapter.rs:570-636 |
| **结论**：在 `TrayMenu::new()` 前后翻转一个 AtomicU8 标志位即可让 hook 对托盘窗口应用专属属性，时序确定、无竞态 | 同上 |
| Slint `global`（Theme/Language）按窗口实例独立，第二窗口拿到的是默认值副本；需在 show 时从 AppWindow 同步 | Slint 语义 |
| AppWindow 已具备全部所需状态：`dark`、`theme-color-index`、`language-options/codes/index`、`current-song: Song{name,liked}`、`playing`；回调：`like-toggle/play-toggle/prev-song/next-song/force-quit/settings-select-changed(int,int)`（id=2 即按索引切语言并持久化） | ui/appwindow.slint:112-179,146,1149-1151；src/handlers/settings.rs:28-36 |
| 语言切换复用 `invoke_settings_select_changed(2, idx)`，无需新逻辑 | settings.rs |
| 无现成 tray-* i18n key，需新增 | ui/language.slint / assets/lang/*.json |
| macOS 透明窗口由后端按 `WindowItem.no_frame()` 或半透明背景自动判定 | winitwindowadapter.rs:565-568 |

## 3. 方案设计

### 3.1 弹窗形态：全屏透明捕获层 + 定位菜单（已选定）

- `TrayMenu` 是独立 `Window`：无边框、透明、置顶、跳任务栏、Windows 下 `no_activate`（不抢前台焦点）。
- 打开时窗口拉伸到**点击点所在显示器全屏**（`available_monitors()` 匹配点击坐标；取不到则主屏），菜单 Rectangle 画在点击点（越界时向上/向左翻转，钳制在屏内 8px 边距）。
- 点击菜单项 → 执行动作 + 关闭；点击菜单外透明区 → 关闭；Esc → 关闭（FocusScope，尽力而为：Windows no_activate 下无键盘焦点，Esc 不生效可接受）。
- 右键托盘时若菜单已开 → 关闭（toggle 语义）。
- 打开菜单期间全屏透明层拦截一切点击 —— 与原生菜单模态行为一致（点击屏他处 = 关闭菜单且点击被吸收）。

备选「小窗 + 失焦关闭」已否决：slint 1.18 无窗口失焦事件钩子，无法可靠实现点击外部关闭。

### 3.2 坐标换算

- 托盘事件给物理坐标；winit 窗口用物理坐标定位；Slint 布局用逻辑坐标。
- Rust 侧：`scale = window.scale_factor()`；`click_logical = (click_phys - monitor_origin) / scale` 写入 `click-x/click-y` 属性；Slint 侧做翻转/钳制（菜单宽高是 Slint 常量，窗口宽高即 root.width/height）。

### 3.3 状态同步（每次 show 时执行，无 stale 风险）

Rust `show_menu()` 在 UI 线程（`invoke_from_event_loop` 内）：
1. 从 AppWindow 读：`current-song.name`（空 → "ZPlayer Music"）、`playing`、`current-song.liked`、`dark`、`theme-color-index`、`language-options`、`language-index`。
2. 从 AppWindow 的 `Language` global 读 8 条 tray-* 文案，写入 TrayMenu 的 `Language` global 实例（两窗口 global 独立）。
3. 写 TrayMenu 属性 + `dark`/`theme-color-index`（组件内 `changed` 回调同步进自己的 Theme 实例）。
4. 定位/拉伸窗口 → `show()` → `refocus()`。

### 3.4 动作接线

TrayMenu 回调 → 复用现有 AppWindow 路径：

| TrayMenu 回调 | 转发 |
|---|---|
| `action-show()` | winit `set_visible(true)+set_minimized(false)+focus_window()`（同现 "show"） |
| `action-fav()` | `u.invoke_like_toggle()` |
| `action-prev/play/next()` | `u.invoke_prev_song() / invoke_play_toggle() / invoke_next_song()` |
| `action-lang(idx)` | `u.invoke_settings_select_changed(2, idx)` |
| `action-quit()` | `u.invoke_force_quit()` |
| `dismiss()` | 隐藏 TrayMenu 窗口 |

### 3.5 i18n 新增 key（zh/en 双语 + language.slint 默认值 + mod.rs apply 写入）

| key | zh | en |
|---|---|---|
| tray-show | 显示 | Show |
| tray-fav | 收藏 | Favorite |
| tray-unfav | 取消收藏 | Unfavorite |
| tray-prev | 上一首 | Previous |
| tray-play | 播放 | Play |
| tray-pause | 暂停 | Pause |
| tray-next | 下一首 | Next |
| tray-language | 语言 | Language |
| tray-quit | 退出 | Quit |

## 4. 文件级改动清单

### 4.1 新增 `ui/components/tray-menu.slint`
- `export component TrayMenu inherits Window`，`background: transparent`。
- 属性：`song-title:string`、`playing:bool`、`liked:bool`、`lang-names:[string]`、`lang-index:int`、`dark:bool`、`theme-color-index:int`、`click-x/click-y:length`。
- 回调：§3.4 七个 + `dismiss()`；公开函数 `refocus()`。
- 布局：全窗 TouchArea（pressed → dismiss）；菜单 Rectangle（宽 160px，圆角 8px，1px divider 描边，drop-shadow，surface-solid 底）位于计算后的 fx/fy；左侧 3px 竖线（palette.primary，rounded，垂直居中、高 60%）；行高 32px，hover → palette.hover 圆角 4；分隔线 1px divider 高 9px 槽位；语言行右侧 "›"；子菜单 Rectangle（宽 132px）贴主菜单右缘（越界改贴左缘），行 = lang-names，当前项前缀 "✓"。
- 翻转逻辑：`fx = min(max(click-x, 8), win-w - menu-w - 8)`；`fy = click-y + menu-h > win-h - 8 ? click-y - menu-h : click-y`；menu-h 由内容行数静态算得（10 行 + 3 分隔 + 上下 padding ≈ 348px，用常量表达式）。
- `init`/`changed` 把 `dark`、`theme-color-index` 同步进本窗口 Theme 实例。
- 文案读本窗口 `Language` global 的 tray-* 属性；播放行 = `playing ? Language.tray-pause : Language.tray-play`；收藏行 = `liked ? Language.tray-unfav : Language.tray-fav`。

### 4.2 `ui/appwindow.slint`
- 顶部加 `import { TrayMenu } from "components/tray-menu.slint";` + `export { TrayMenu }`，使组件编入同一模块供 Rust `crate::TrayMenu` 使用。

### 4.3 `ui/language.slint`
- 在「通用」段后新增「托盘菜单」段，9 个 `in-out property <string> tray-*` 带中文默认值（见 §3.5）。

### 4.4 `ui/assets/lang/zh.json` / `en.json`
- 各加 9 个 key（值见 §3.5）。

### 4.5 `src/i18n/mod.rs`
- `apply()` 末尾加 9 行 `l.set_tray_*(g("tray-*"));`。

### 4.6 `src/platform/tray.rs`（重写核心）
- 移除原生菜单构建（`Menu/MenuItem` 全部删除，不再 `with_menu`）；保留图标 + tooltip + `with_menu_on_left_click(false)`。
- 新增 `static NEXT_IS_TRAY: AtomicU8` + `pub(crate) fn take_tray_flag() -> bool`（swap 0）；`pub(crate) fn apply_tray_attrs(attrs) -> attrs`：decorations(false)/transparent(true)/resizable(false)/skip_taskbar(true)/window_level(AlwaysOnTop)/visible(false)，Windows 追加 `with_no_activate(true)`（`winit::platform::windows::WindowAttributesExtWindows`）。
- `init(app, lang)` 签名保留（macOS/Linux 同用）；init 末尾创建 `TrayMenu::new()`（创建前置位 NEXT_IS_TRAY=1，创建后归零），接线 7 个回调（clone Weak<AppWindow> 转发，同现有 `forward` 语义），实例 `mem::forget` 常驻。
- 点击线程：Left+Up → `"toggle"`（不变）；Right+Up → 带 `ev.position` 调 `show_menu(app_weak, phys_x, phys_y)`。
- `show_menu`：`invoke_from_event_loop` 内 → 若 TrayMenu 已可见则 hide 返回；否则 §3.3 同步 → `menu.window().with_winit_window()` 拿 scale/monitors → 定位拉伸 → `show()` → `refocus()`。TrayMenu 实例经 `thread_local!`/`OnceLock<Weak<TrayMenu>>`（UI 线程单例，mem::forget 后存 Weak 于 static Mutex）。
- cfg 门控从 Windows 扩到 `any(windows, macos, linux)`。

### 4.7 `src/platform/mod.rs`
- `#[cfg(windows)] pub mod tray;` → `#[cfg(any(target_os="windows", target_os="macos", target_os="linux"))] pub mod tray;`（drop_files 保持 Windows-only）。

### 4.8 `Cargo.toml`
- `tray-icon = "0.26"` 从 `[target.'cfg(windows)'.dependencies]` 移到 `[target.'cfg(any(target_os="windows", target_os="macos", target_os="linux"))'.dependencies]`。

### 4.9 `src/main.rs`
- hook 闭包首行：`if crate::platform::tray::take_tray_flag() { return crate::platform::tray::apply_tray_attrs(attrs); }`（两个 hook 定义点都加；cfg 门控与 tray 模块一致，wasm 路径不引用）。
- 托盘 init 调用点（约 422 行）：`#[cfg(windows)]` → `#[cfg(any(target_os="windows", target_os="macos", target_os="linux"))]`。

### 4.10 `.github/workflows/release.yml`（build-macos 段打包后追加）
在现有「打包」步骤后新增「打包 .app bundle」步骤（bash）：
1. `mkdir -p "ZPlayer Music.app/Contents/MacOS" "…/Contents/Resources"`
2. 二进制放 `Contents/MacOS/zplayer-music`（已 strip+codesign 的那份）
3. 写 `Contents/Info.plist`：CFBundleName/DisplayName=ZPlayer Music、CFBundleIdentifier=com.zplayer.music、CFBundleExecutable=zplayer-music、CFBundleIconFile=icon、CFBundleVersion/ShortVersionString=矩阵版本号（从 tag 推导）、LSMinimumSystemVersion=11.0、NSHighResolutionCapable=true
4. 图标：`sips` 把 `ui/assets/app-icon.png` 缩出 16/32/64/128/256/512/1024 及 @2x 各尺寸进临时 `.iconset`，`iconutil -c icns` 生成 `Contents/Resources/icon.icns`
5. `codesign --force --deep --sign - "ZPlayer Music.app"`（ad-hoc 深签名）
6. `tar -czf zplayer-music-macos-<arch>.app.tar.gz "ZPlayer Music.app"`；上传 artifact 改为 .app 包（保留原裸二进制 tar 以便更新器继续使用 —— 更新器替换的是 bundle 内二进制，发布物两者并存）

注意：在线更新（src/network/update.rs）在 macOS 上的替换目标将变为 `.app/Contents/MacOS/zplayer-music`；本次只改打包，更新器 macOS 路径适配列为后续遗留项（当前 macOS 更新器本就无法对应裸二进制分发，不属退化）。

## 5. 验证

1. `cargo check`（Windows 本机）通过。
2. `cargo build` 后运行：右键托盘 → 自绘菜单出现在点击点（含越界翻转）；hover 高亮；点「语言 ›」出子菜单并切换生效（界面文案变更、配置持久化）；收藏/播放控制/显示/退出均生效；点菜单外任意处关闭；任务栏无托盘菜单窗口按钮。
3. 深色模式下菜单配色跟随主题。
4. macOS：CI 出包后用户实测 .app（icon 正常、无终端窗口、状态栏托盘出现且右键为自绘菜单）。
5. Linux 构建不受影响（tray-icon 构建失败风险低；运行时无 StatusNotifier 环境 `build()` 失败仅 warn，不影响主程序）。

## 6. 风险与回退

- 全屏透明层在个别 Windows 机器上若透明合成异常（软渲染 + 透明），表现为全屏黑/白一帧 —— 回退方案：窗口改非全屏小菜单窗 + 失焦轮询关闭。本版不预埋。
- 托盘窗口复用单例，避免重复 hook 标志与重复创建开销。
- 所有改动集中在新增文件 + 枚举清楚的修改点，回退 = git revert 单个 commit。
