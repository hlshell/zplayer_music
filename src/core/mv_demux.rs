//! Demux 主循环（内嵌自 player-core/src/decoder/demux.rs）。
//! av_read_frame 拉流 → 分发到 video 包队列；处理 seek 命令和 pause/shutdown。

use crate::core::mv_config::MvPlaybackConfig;
use crate::core::mv_decoder::MvDemuxCommand;
use crate::core::mv_state::MvPlaybackState;
use ffmpeg_next as ffmpeg;
// Packet::as_mut_ptr 来自该 trait（手动 av_read_frame 循环需要）
use ffmpeg_next::packet::Mut as _;
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// 带 seek epoch 的包（解码线程丢弃过期 epoch 的包）。
pub type MvTaggedPacket = (u64, ffmpeg::Packet);

#[allow(clippy::too_many_arguments)]
pub fn mv_run_demux(
    ictx: &mut ffmpeg::format::context::Input,
    vindex: usize,
    vtx: &mpsc::SyncSender<MvTaggedPacket>,
    aindex: Option<usize>,
    atx: Option<&mpsc::SyncSender<MvTaggedPacket>>,
    state: &Arc<MvPlaybackState>,
    _cfg: &MvPlaybackConfig,
    cmd_rx: &mpsc::Receiver<MvDemuxCommand>,
) -> Result<(), String> {
    let start_offset_us = {
        let raw = unsafe { (*ictx.as_ptr()).start_time };
        if raw == ffmpeg::ffi::AV_NOPTS_VALUE || raw < 0 {
            0
        } else {
            raw
        }
    };

    // 连续读包错误计数：网络中断/超时等非 EOF 错误给予短窗口重试，
    // 超过上限判定为"流中断"上报（上层重连当前 MV），绝不当自然播完。
    let mut read_errors: u32 = 0;
    const MAX_READ_ERRORS: u32 = 8;

    'session: loop {
        let mut pending_seek: Option<u64> = None;
        'packets: loop {
            // 注意：暂停时不再停读——继续 av_read_frame 填满包队列（预缓冲），
            // 队列满时由下方的 try_send 重试循环背压；恢复播放即可秒开。
            if state.shutdown() {
                break 'session;
            }
            if let Some(t) = mv_poll_seek(cmd_rx) {
                pending_seek = Some(t);
                break 'packets;
            }
            // 手动 av_read_frame：不能用 ictx.packets() 迭代器——它把任何错误
            // （网络中断/超时/HTTP 错误）都吞成"迭代结束"，会被误判为自然播完
            // 而触发自动连播下一首（"播着播着突然跳到下一个 MV"的根因）。
            let mut packet = ffmpeg::Packet::empty();
            let ret = unsafe { ffmpeg::ffi::av_read_frame(ictx.as_mut_ptr(), packet.as_mut_ptr()) };
            if ret < 0 {
                if ret == ffmpeg::ffi::AVERROR_EOF {
                    break 'packets; // 真 EOF：自然播完
                }
                read_errors += 1;
                if read_errors > MAX_READ_ERRORS {
                    let msg = format!("读取流中断（av_read_frame 错误码 {ret}）");
                    log::warn!(target: "mv_demux", "{msg}");
                    return Err(msg);
                }
                // 短暂重试窗口：部分抖动可恢复
                std::thread::sleep(Duration::from_millis(250));
                continue 'packets;
            }
            read_errors = 0;
            let epoch = state.epoch();
            let sidx = packet.stream();
            let pkt_pts = packet.pts();
            let Some(stream) = ictx.stream(sidx) else { continue 'packets };
            let pkt_tb = stream.time_base();
            if sidx == vindex {
                // 视频包不允许丢：队列满时短睡重试（暂停/解码 pacing 背压时常见），
                // 重试间隙轮询 seek/shutdown——否则暂停中拖进度条会因 send 永久阻塞而卡死。
                let mut pair: MvTaggedPacket = (epoch, packet);
                loop {
                    match vtx.try_send(pair) {
                        Ok(()) => break,
                        Err(mpsc::TrySendError::Disconnected(_)) => break 'session,
                        Err(mpsc::TrySendError::Full(p)) => {
                            pair = p;
                            if state.shutdown() {
                                break 'session;
                            }
                            if let Some(t) = mv_poll_seek(cmd_rx) {
                                pending_seek = Some(t);
                                break 'packets;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                    }
                }
            } else if Some(sidx) == aindex {
                // 音频包：队列满时等待（不丢包），背压只让 demux 暂停读取，不破坏音频时间轴。
                if let Some(atx) = atx {
                    let mut pair: MvTaggedPacket = (epoch, packet);
                    loop {
                        match atx.try_send(pair) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Disconnected(_)) => break 'session,
                            Err(mpsc::TrySendError::Full(p)) => {
                                // 音频包绝不丢弃：丢包会让音频内容比时间轴短，
                                // 而音频时钟按"渲染样本数"推进——每丢一包就永久少 23ms，
                                // 累积后表现为"播着播着突然音画不同步 + 杂音"。
                                // 队列满只是预缓冲饱和，等待即可（等待间隙仍响应 seek/shutdown）。
                                pair = p;
                                if state.shutdown() {
                                    break 'session;
                                }
                                if let Some(t) = mv_poll_seek(cmd_rx) {
                                    pending_seek = Some(t);
                                    break 'packets;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                }
            }
            // 预加载水位：读入到哪个时间点就记录到哪（首次进入/seek 后从当前位置重新累积），
            // 供进度条绘制"已缓冲"区间。
            mv_note_buffered(state, pkt_pts, pkt_tb, mv_seek_base(state, start_offset_us));
        }
        match pending_seek {
            Some(target_us) => {
                // 偏移基准：优先用会话首帧 PTS（部分流首帧 PTS 非零，仅用 stream
                // start_time 会把目标算早，表现为"快进变后退"）；首帧未解出时回退。
                let base = mv_seek_base(state, start_offset_us);
                let ts = base.saturating_add(target_us as i64);
                // 记录目标点：seek 通常落在目标之前的最近关键帧，
                // 解码线程据此快速丢弃早于目标的帧/采样，避免“点了进度条却从关键帧重播”。
                state.set_seek_target_us(target_us);
                let _ = state.bump_epoch();
                // 目标在已缓冲水位内：数据仍在本地代理分片缓存，demux 重读很快追平，
                // 保持水位显示——避免 UI 缓冲条清空 + 门控误触发"重新缓冲"转圈。
                // 只有拖出水位（数据确实未下载）才重置水位，让门控正常进入缓冲态。
                // （note_buffered_us 是 fetch_max 语义，重读追平旧水位后继续增长，显示连续）
                if target_us > state.buffered_us() {
                    state.set_buffered_us(target_us);
                }
                match ictx.seek(ts, ..ts) {
                    Ok(()) => {
                        log::info!(target: "mv_demux",
                            "seek to {}ms (epoch {})", target_us / 1000, state.epoch())
                    }
                    Err(e) => log::warn!(target: "mv_demux", "seek failed: {e}"),
                }
            }
            None => break 'session,
        }
    }
    Ok(())
}

/// 时间轴基准（微秒）：首帧 PTS 已锁定则用它，否则回退容器 start_time。
fn mv_seek_base(state: &Arc<MvPlaybackState>, start_offset_us: i64) -> i64 {
    if state.first_pts_ready() { state.first_pts_us() as i64 } else { start_offset_us.max(0) }
}

/// 用包 PTS 刷新预加载水位（相对时间轴；NOPTS 直接忽略）。
fn mv_note_buffered(state: &Arc<MvPlaybackState>, pts: Option<i64>, tb: ffmpeg::Rational, base_us: i64) {
    let Some(pts) = pts else { return };
    let Some(abs_us) = crate::core::mv_decoder::mv_pts_to_us(pts, tb.0, tb.1) else { return };
    let rel = abs_us - base_us;
    if rel >= 0 { state.note_buffered_us(rel as u64); }
}

/// 排空命令通道，只保留最后一个 seek（slider 拖拽可能叠多个请求）。
fn mv_poll_seek(cmd_rx: &mpsc::Receiver<MvDemuxCommand>) -> Option<u64> {
    let mut target = None;
    while let Ok(cmd) = cmd_rx.try_recv() {
        match cmd {
            MvDemuxCommand::Seek(us) => target = Some(us),
        }
    }
    target
}
