# ZPlayer Music 移动端验收记录

日期：2026-11-22  
范围：`docs/superpowers/specs/2026-11-22-mobile-ui-design.md` 与 `docs/superpowers/plans/2026-11-22-mobile-ui.md` 阶段 0–7。

> 依据 spec §9.2：「未实际构建或运行的平台不得标记为验证通过」。本机为 Windows，无 Android SDK/NDK、无 Android/iOS 设备、无 macOS/Xcode，因此 Android/iOS 真机项全部标记为**未验证**，不以编译通过代替。

## 1. 本地已执行的验证

| 项目 | 命令 | 结果 |
| --- | --- | --- |
| 移动 feature 编译 | `cargo +stable-x86_64-pc-windows-msvc check --no-default-features --features mobile` | 通过（exit 0） |
| 桌面编译回归 | `cargo +stable-x86_64-pc-windows-msvc check` | 通过（exit 0） |
| 桌面测试回归 | `cargo +stable-x86_64-pc-windows-msvc test` | 通过：4 + 4 + 6 passed，0 failed |
| 移动 feature 单测 | `cargo +stable-x86_64-pc-windows-msvc test --no-default-features --features mobile --lib --tests` | 通过（布局分类器 6 项） |
| CI 工作流 YAML | `python -c "import yaml; yaml.safe_load(...)"` | 解析通过 |

说明：本机 check 仅证明 Rust/Slint 代码与移动 UI 文件可编译，不代表 Android/iOS 原生宿主可构建或可运行。

## 2. 验收标准逐项状态

| # | 标准（spec §9.3） | 实现状态 | 验证状态 |
| --- | --- | --- | --- |
| 1 | 首页真实数据、进详情并播放 | 已实现 | 未验证（需设备/CI） |
| 2 | 搜索单曲/专辑/歌单、详情返回、快速操作不串台 | 已实现（`search_seq`/`detail_seq` 防串） | 未验证（需设备/CI） |
| 3 | 播放/暂停/上下首/模式/进度拖动/队列 | 已实现 | 未验证（需设备/CI） |
| 4 | 旋转/改尺寸不中断播放、不丢状态 | 已实现（Slint 三形态重排，状态在 AppWindow） | 未验证（需设备/CI） |
| 5 | 设置立即生效、重启恢复 | 已实现（写 config.toml + 即时回写） | 未验证（需设备/CI） |
| 6 | 网络失败/无结果/封面失败/歌词缺失/播放失败有反馈 | 已实现（错误文案、空状态、失败降级） | 未验证（需设备/CI） |
| 7 | 横屏窄侧栏、平板完整侧栏、系统区域不遮挡 | 已实现（含 `Window.safe-area-insets` 注入与安全区内边距） | 未验证（需设备/CI） |
| 8 | iOS 26 封面取色与玻璃、性能降级 | 已实现（封面平均色 + 静态半透明降级；无实时模糊） | 未验证（需 iOS 26 真机） |
| 9 | Android 保持 Material，不出现 iOS 玻璃 | 已实现（`platform==1` 切换不透明表面/阴影） | 未验证（需 Android 真机） |
| 10 | 桌面与 Web 行为不变 | 桌面编译与测试通过 | 桌面已验证；Web 未执行 |

## 3. 阻塞项与原因

- **Android 真机**：本机无 Android SDK/NDK、无 arm64 设备；CI 工作流尚未在 GitHub 上实际运行。
- **iOS 真机**：需 macOS + Xcode + 签名；CI 的 `check-ios` 仅做无签名 Simulator 编译校验。
- **平板/横屏/刘海屏/手势条**：需对应设备；模拟器只能辅助布局检查，不能替代真机验收。
- **iOS 26 玻璃视觉与性能**：需 iOS 26 设备确认渲染与帧率。

## 4. 后续补齐步骤

1. 推送仓库后在 Actions 手动运行 `Mobile` 工作流：
   - `check-android`：确认 Debug APK 产物可安装并冒烟（首页/搜索/播放/详情/设置）。
   - `check-ios`：确认 macOS Runner 上 XcodeGen + xcodebuild 无签名构建通过。
2. 配置签名 Secrets（`ANDROID_KEYSTORE_*`、`IOS_CERT_*`、`IOS_PROFILE_BASE64`、`IOS_TEAM_ID`）后手动触发，验证签名 APK/IPA 产物。
3. 按 spec §9.2 设备矩阵逐项执行第 2 节验收标准，并将结果回填本文件。
4. 真机重点回归：
   - 快速连点不同歌曲/快速切换详情，确认旧响应不覆盖；
   - 播放中旋转屏幕与平板改尺寸，确认播放与页面状态保持；
   - 刘海/灵动岛/手势条设备确认顶部与底部不被遮挡；
   - 无网络、封面 404、无歌词、播放失败四种异常路径的界面反馈。
