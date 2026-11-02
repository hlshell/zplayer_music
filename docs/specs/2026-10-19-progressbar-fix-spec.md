# 播放进度条修复规格（进度起点 / 悬停时间浮窗 / 时间错乱）

日期：2026-10-19
状态：待确认

## 背景与问题

用户报告三个问题（附截图）：

1. **时间显示数值错乱**：截图显示 `0.08333333360:05 / 02:58`。
2. **进度条悬停任意位置无时间浮窗**。
3. **进度条行为不像常规进度条**（起点/点击跳转异常）。

## 根因（已从源码逐一确认）

### R1 时间错乱 — Slint 浮点除法拼串
`ui/components/playbar.slint` L171-173（已播时间）与 L309-311（悬停提示）：
`(elapsed-sec / 60)` 中 int/int 在 Slint 产生 f32（5/60 = 0.0833…），直接与字符串拼接。
另外三元分支文案 `"0:0"` / `":"` 语义混乱（分钟与秒之间插入了错误前缀）。

### R2 续播进度映射错误 — rodio get_pos() 不含 skip 量（核心）
- `src/player.rs` `play_at()` 用 `decoder.skip_duration(offset)` 实现续播。
- rodio 0.19 `Sink::append()`（sink.rs L122-129）在用户源外再包 `.track_position()`；
  `TrackPosition::get_pos()`（position.rs L66-72）= 实际流过采样数换算时间 + offset_duration(默认 0)，
  **不包含 skip_duration 跳过的时长**。
- 后果：
  - 续播后 `status()` 返回的 progress 从 0 重新计数（进度条/时间与真实音频位置脱节）；
  - 退出时 `save_state()` 保存 `progress_sec = p * duration`（p 被低估）→
    **续播位置跨会话逐次缩水**（如 96s → 5s → …），与截图现象吻合。
- 修复方向：`Shared` 增加 `base_sec`（= offset_sec），`status()` 计算
  `pos = base_sec + sink.get_pos()`，再 `/ total_sec`。

### R3 悬停浮窗不显示 — 触摸区仅 4px 高
`prog-ta` 在 4px 高的进度条内，鼠标极难命中（浮窗本身无裁剪问题：
PlayBar 是根 VerticalLayout 直接子元素，上方无 clip 祖先；tooltip 声明于 PlayBar 末尾 z 序最高）。

### R4 点击进度条无真实跳转
`prog-ta.clicked` 只写 `root.progress` 视觉值，250ms 轮询定时器随即用真实位置覆盖
（rodio Sink 不支持运行中 seek）。需通过"重播实现跳转"：
缓存命中时重解码+skip 近乎即时（歌曲播放时必已写入缓存，同会话内 seek 全部命中缓存）。

### R5 播放按钮边界失效
`main.rs` `on_play_toggle`：无活动播放且不满足续播条件时调用 `player::toggle()`——
sink 为空时 no-op（首次启动未播放过 / 歌曲播完后 CURRENT 已清空）→ 按钮无响应。
且播完后 `last_progress ≈ duration` 仍满足续播条件 → skip 到曲尾 → 静音假播。

## 修改方案

### M1 `ui/components/playbar.slint`
1. 新增格式化函数（分钟取整、秒补零）：
   ```slint
   function fmt-sec(s: int) -> string {
       return Math.floor(s / 60) + ":" + (Math.mod(s, 60) < 10 ? "0" : "") + Math.mod(s, 60);
   }
   ```
   - 已播时间 Text：`text: root.fmt-sec(root.elapsed-sec);`
   - 悬停提示 Text：`text: root.fmt-sec(root.hover-sec);`
2. 触摸区加高：`prog-ta` 从 4px 条内移出为 PlayBar 顶部全宽 `height: 16px`（y:0），
   覆盖进度条 + 内容行顶部空白；z 序在内容行控件（封面/播放键等）之下，不影响其点击。
   - `clicked`：先 `root.progress = f`（即时视觉反馈），再 `root.seek-fraction(f);`
   - `hover-sec` 计算后钳制到 `0..duration-sec`。
3. 悬停浮窗保持 PlayBar 上方（y:-28px，无裁剪风险），可见条件不变，
   文本改用 `fmt-sec`。

### M2 `src/player.rs`
- `Shared` 增加 `base_sec: f32`；`play_at()` 记录 `base_sec = offset_sec`。
- `status()`：`let pos = s.base_sec + s.sink.get_pos().as_secs_f32();`
  返回 `((pos / s.total_sec).clamp(0.0, 1.0), playing)`。
- 效果：续播后进度条从续播点开始、时间显示真实音频位置、退出保存的进度不再缩水。

### M3 `ui/appwindow.slint`
- 根新增 `callback seek(float);`；PlayBar 实例接线 `seek-fraction(f) => { root.seek(f); }`。

### M4 `src/main.rs`
1. 新增 `ui.on_seek(f)`：
   - `f = f.clamp(0.0, 1.0)`；取当前曲目 `cur`，`target = f * cur.duration_sec`；
     `target < 1.0` 时按 0 处理。
   - 无活动播放也允许：直接从该位置开播（常规进度条语义）。
   - 调 `resolver_cb(cur.id as u32 as i64, name, artist, dur, target)` + `u.set_playing(true)`。
     （重播实现跳转；同会话内歌曲已在缓存，几乎即时）
2. `on_play_toggle` 修正：
   - 续播条件追加 `&& last_progress < last_dur - 3.0`（播完重播从头开始）；
   - 不满足续播的"无活动"分支：`resolver_cb(cur_uid, cur.name…, cur.duration_sec, 0.0)`
     + `set_playing(true)` 替换 `toggle()`（首次/播完后按钮恢复可用）。

## 验证步骤

1. `cargo build` 无错误无新增警告。
2. 运行应用：
   - 播放任一歌曲 → 时间显示 `m:ss` 正常递增、无浮点串；
   - 鼠标悬停进度条区域任意位置 → 出现浮窗显示对应时间，文本格式正确；
   - 点击进度条中部 → 音频从该位置附近继续，进度条/时间与位置一致（不回弹）；
   - 播放至中途退出 → 重开点播放 → 续播点与上次一致（进度条从该处开始且时间正确）；
   - 播完一曲后再点播放 → 从头播放（非静音假播）。

## 影响范围

- `ui/components/playbar.slint`、`ui/appwindow.slint`、`src/player.rs`、`src/main.rs`
- 不改动缓存/配置/日志逻辑；`save_state` 无需变更（R2 修复后 p 即为真实比例）。
