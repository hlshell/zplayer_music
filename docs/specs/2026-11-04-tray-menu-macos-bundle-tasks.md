# 任务清单 — 托盘自绘右键菜单 + macOS .app 打包

对应规格：2026-11-04-tray-menu-macos-bundle-spec.md

## A. 托盘自绘弹窗

- [x] A1. 新增 `ui/components/tray-menu.slint`（TrayMenu 组件：全屏透明捕获层 + 定位菜单 + 语言子菜单，属性/回调/翻转逻辑按规格 §4.1）
- [x] A2. `ui/appwindow.slint` 顶部 import + `export { TrayMenu }`
- [x] A3. `ui/language.slint` 新增 9 个 tray-* 属性（中文默认值）
- [x] A4. `ui/assets/lang/zh.json`、`en.json` 各加 9 个 key
- [x] A5. `src/i18n/mod.rs` `apply()` 末尾加 9 行 set_tray_*
- [x] A6. 重写 `src/platform/tray.rs`：去原生菜单、Right+Up → show_menu(物理坐标)、TrayMenu 单例创建（hook 标志 + mem::forget + Weak 存 static）、7 回调接线、show 时状态同步 + 多显示器定位 + cfg 扩三平台
- [x] A7. `src/platform/mod.rs` tray 模块 cfg 扩 `any(windows, macos, linux)`
- [x] A8. `Cargo.toml` tray-icon 依赖移到 `cfg(any(windows, macos, linux))` target 段
- [x] A9. `src/main.rs` 两处 hook 闭包加托盘分支；托盘 init 调用点 cfg 放宽

## B. macOS .app 打包

- [x] B1. `.github/workflows/release.yml` build-macos 追加 .app bundle 步骤（目录结构 + Info.plist + sips/iconutil 生成 icon.icns + codesign --deep + tar.gz），artifact 上传 .app 包（保留裸二进制 tar）

## C. 验证

- [x] C1. `cargo check` 通过
- [ ] C2. 本地运行实测：右键弹自绘菜单（定位/翻转/hover/子菜单/收藏/播放控制/语言切换/退出/点外关闭/无任务栏按钮）
- [ ] C3. 深浅主题下菜单配色正确
- [ ] C4. 推送后用户 macOS 实测 .app 三问题修复
