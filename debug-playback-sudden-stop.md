# Debug Session: playback-sudden-stop

- 状态：插桩版本构建成功，等待用户复现
- 日期：2026-11-22
- 项目根目录：`k:\MusicPlayer\slint-demo`
- 症状：歌曲播放过程中突然停止，可能伴随重试或自动切歌。
- 预期：除非自然结束、用户控制、设备切换或明确错误，当前歌曲应持续播放。
- 当前阶段：静态分诊完成，尚未修改播放逻辑，尚无运行时结论。

## 证据位置

- DEBUG_EVIDENCE_DIR：`C:\Users\93453\AppData\Local\Temp\trae-debug\slint-demo\playback-sudden-stop`
- NDJSON：`C:\Users\93453\AppData\Local\Temp\trae-debug\slint-demo\playback-sudden-stop\trae-debug-log-playback-sudden-stop.ndjson`
- ENV：`C:\Users\93453\AppData\Local\Temp\trae-debug\slint-demo\playback-sudden-stop\playback-sudden-stop.env`

## 可证伪假设

| ID | 假设 | 关键观察点 | 状态 |
| --- | --- | --- | --- |
| H1 | `Sink::empty()` 在歌曲实际结束前提前为真，被误判为自然结束 | empty、FINISHED、base_sec、sink_pos、total_sec、流代号 | 待验证 |
| H2 | 非预期的 `stop()`、`toggle()` 或输出设备切换导致暂停/清流 | 各控制入口、调用时状态、位置、线程与流代号 | 待验证 |
| H3 | 重叠 `play_at()`、自动重试或旧音频线程竞争清理新流状态 | 每次 play_at/stop 的流代号、CURRENT 替换、SWITCHING、FINISHED | 待验证 |
| H4 | 输出流、Sink 或 Decoder 创建失败后提前返回，未向上层报告错误 | 三个创建阶段的成功/失败、CURRENT 与 UI playing | 待验证 |
| H5 | 250ms 轮询把短暂 inactive 误判为故障，触发重试及自动下一首 | active 跳变、auto_was_playing、finished、retry_cnt、重试/切歌分支 | 待验证 |

## 已知静态事实

- 桌面音频通过 rodio Sink 播放完整下载的字节；位置由 `base_sec + sink.get_pos()` 得出。
- 音频线程每 100ms 检查 `sink.empty()`；当前流为空时设置 FINISHED 并清除 CURRENT。
- UI 每 250ms 轮询；曾播放的流变为 inactive 后，按 FINISHED 选择自然下一首，否则最多重试两次再下一首。
- `play_at()` 内输出流、Sink、Decoder 创建失败路径当前会直接返回。
- 以上事实不能单独证明根因，必须由运行时事件确认。

## 插桩计划

1. 为每次音频流分配调试代号，记录 play_at、输出流/Sink/Decoder 创建、CURRENT 替换与线程退出。
2. 记录 stop、toggle、输出设备切换，以及暂停、empty、进度、SWITCHING/FINISHED 快照。
3. 在 250ms 轮询中仅记录状态跳变、重试和自动切歌，避免每轮高频上报。
4. 记录 resolver 获取音频成功/失败和播放入口，关联歌曲与流代号。

## 运行时证据

- HTTP 收集器：`http://127.0.0.1:7777`，health 返回 `status=ok`，探针事件已成功写入并可通过 `/logs?last=1` 读回。
- 插桩范围：`desktop.rs` 的流创建/注册/退出、stop/toggle/设备切换；`play.rs` 的轮询状态跳变和自动决策；`player/mod.rs` 的音频解析成功/失败。
- 运行标识：默认 `pre-fix`，可通过 `TRAE_DEBUG_RUN_ID` 切换；每次音频流带单调递增 `stream_id`。
- 插桩版本：dev 构建成功（退出码 0）；release 版（target/release，构建于 11/22 02:47）二进制内检出全部插桩标记串，确认带插桩。
- 实际复现事件：2026-11-23 01:48–01:58 采集到 3 首歌完整播放序列（sid=4/5/6）。前两首正常播完并自动切歌；第三首 sid=6（歌曲 1881426227，总时长 166s）播完后未切歌、停在当前曲。

### 关键事件序列（sid=6，已证实根因）

| 时间 | 事件 | 关键字段 |
| --- | --- | --- |
| 01:55:53.825 | H3 stream registered | sid=6 active=True |
| 01:55:54.014 | H5 polling state changed | sid=6 active=True play=True awp=False→后续每轮 awp=True |
| **01:58:40.126** | **H5 polling state changed** | **sid=6 active=True play=False paused=False empty=True fin=False pos=166.15/166 awp=True** |
| 01:58:40.155 | H1 audio thread observed empty sink | sid=6 is_current=True fin=True（音频线程此时才置 FINISHED 并清 CURRENT） |
| **01:58:40.376** | **H5 polling state changed** | **sid=0 active=False fin=True awp=False** —— 落入 else 分支，未触发 auto next |

对照：前两首（sid=4/5）的 H1（音频线程清 CURRENT）均先于 UI 轮询看到 active=False，awp 保持 True，故正常切歌。第三首 UI 轮询恰好先踩中「sink 已空但 CURRENT 未清」窗口。

## 根因与修复

**根因（H5 证实，H1 时序佐证）：UI 250ms 轮询与音频线程清理之间的竞态。**

- 音频线程每 100ms 检测 `sink.empty()`，检测到后置 `FINISHED=true` 并清空 `CURRENT`（事件 27）。
- UI 轮询每 250ms 读取状态。当它在音频线程清理之前的窗口内读到 `active()=True`（CURRENT 未清）但 `status().playing=False`（sink 已 empty）时，原逻辑执行 `auto_wp.set(playing)` 把 `auto_was_playing` 清零（事件 26）。
- 下一轮（事件 28）UI 看到 `active=False, finished=True`，但 `auto_wp` 已为 False，落入 `else { auto_wp.set(false) }` 分支，不触发「播完自动下一首」，歌曲停在当前曲不动。
- 因为是时序竞态，复现概率性出现，表现为「不固定、各种来源都可能」；用户感知为「播完没切下一首/自动停止」。

**修复：**
1. `desktop.rs` / `common_rodio.rs` / `web.rs` 新增门面函数 `paused()`（mobile 通过 re-export 同步），返回当前 sink/元素是否处于用户暂停态。
2. `play.rs` 进度轮询 `active()` 分支不再无条件 `auto_wp.set(playing)`：
   - `playing=true` → 置 awp=true、复位重试计数、刷播放态；
   - `playing=false && !paused` → 判定为「sink 已空、CURRENT 未清的结束过渡态」，**保留 awp=true 且不刷暂停态**，让下一轮 active=false 时按 finished 正常切歌；
   - `playing=false && paused` → 用户主动暂停，清零 awp 并刷暂停态。
3. 附带避免结束过渡态播放图标闪烁为暂停。

## 前后验证

待 release 构建完成、用户重启复现后填写：预期事件序列中 H1 sink empty 之后应紧跟 H5 `auto next after finished`，不再出现 `active=False fin=True awp=False` 后停住的情况。
