# 任务清单：播放进度条修复

对应规格：`docs/specs/2026-10-19-progressbar-fix-spec.md`

## T1 player.rs — 续播进度基准（R2，核心）
- [x] `Shared` 增加 `base_sec: f32`
- [x] `play_at()` 记录 `base_sec = offset_sec`
- [x] `status()` 返回 `((base_sec + get_pos()) / total_sec).clamp(0,1)`
- 验证：cargo build 通过

## T2 playbar.slint — 时间格式 + 触摸区 + 浮窗（R1/R3）
- [x] 新增 `fmt-sec(s: int) -> string`（floor 分钟 + 补零秒）
- [x] 已播时间与悬停提示均改用 `fmt-sec`
- [x] `prog-ta` 加高为 PlayBar 顶部全宽 16px（覆盖 4px 条 + 内容行顶部）
- [x] `hover-sec` 钳制 `0..duration-sec`
- [x] 新增 `callback seek-fraction(float)`；clicked：设视觉 progress + 发 seek-fraction
- 验证：cargo build 通过

## T3 appwindow.slint — seek 接线（M3）
- [x] 根新增 `callback seek(float)`
- [x] PlayBar 实例 `seek-fraction(f) => { root.seek(f); }`
- 验证：cargo build 通过

## T4 main.rs — on_seek + on_play_toggle 修正（R4/R5）
- [x] `on_seek(f)`：钳制 → target=f*dur（<1s 按 0）→ resolver 重播 + set_playing(true)
- [x] 续播条件追加 `last_progress < last_dur - 3.0`
- [x] 无活动不续播分支改为 resolver 从 0 播放（替换 toggle() no-op）
- 验证：cargo build 通过

## T5 整体验证
- [x] cargo build 干净通过（EXIT=0，零新增警告）
- [x] 应用启动验证：进程存活、Responding=True，无崩溃
      （时间格式/悬停浮窗/点击跳转/续播点不缩水/播完重播 待用户实际操作确认）
