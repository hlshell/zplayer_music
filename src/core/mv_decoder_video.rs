//! 视频解码线程（内嵌自 player-core/src/decoder/video.rs）。
//! 有音频时钟时 pacing 跟音频走（晚于 PTS late_drop_us 的帧直接丢弃追时钟）；
//! 无音频/音频时钟未就绪时回退 wall-clock pacing。

use crate::core::mv_audio::MvAudioClock;
use crate::core::mv_config::MvPlaybackConfig;
use crate::core::mv_decoder::mv_pts_to_us;
use crate::core::mv_decoder_hw::mv_try_enable_hw_decoder;
use crate::core::mv_error::MvResult;
use crate::core::mv_frame::{MvFrame, MvFrameBuffer};
use crate::core::mv_state::MvPlaybackState;

use ffmpeg::codec::context::Context;
use ffmpeg::software::scaling::{Context as Scaler, Flags};
use ffmpeg::util::format::Pixel;
use ffmpeg::util::frame::Video;
use ffmpeg_next as ffmpeg;

use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use ffmpeg::ffi::av_hwframe_transfer_data;

pub fn mv_video_decode_thread(
    rx: mpsc::Receiver<crate::core::mv_demux::MvTaggedPacket>,
    vparams: ffmpeg::codec::Parameters,
    vtb: ffmpeg::Rational,
    buffer: MvFrameBuffer,
    clock: Option<MvAudioClock>, // 音频时钟；None 时 wall-clock pacing
    state: Arc<MvPlaybackState>,
    cfg: MvPlaybackConfig,
) -> MvResult<()> {
    // 尝试硬解
    let ctx = Context::from_parameters(vparams)?;
    let codec_id = ctx.id();
    let codec = ffmpeg::decoder::find(codec_id).ok_or(crate::core::mv_error::MvPlayerError::DecoderNotFound)?;
    let mut dec_wrap = ctx.decoder();
    let hw_pix_fmt: Option<i32> = unsafe { mv_try_enable_hw_decoder(dec_wrap.as_mut_ptr(), codec.as_ptr()) };
    let mut vdec = dec_wrap.open_as(codec).and_then(|o| o.video())?;

    // 输出尺寸（cfg.target_width 有值就用，否则原分辨率）
    let (out_w, out_h) = if cfg.target_width > 0 && cfg.target_height > 0 {
        (cfg.target_width, cfg.target_height)
    } else {
        (vdec.width(), vdec.height())
    };
    let out_pix = match cfg.output_format {
        crate::core::mv_config::MvPixelFormat::Rgba => Pixel::RGBA,
    };

    let mut scaler: Option<(Scaler, Pixel, u32, u32)> = None;
    let mut vyuv = Video::empty();
    let mut vout = Video::empty();

    let mut pace = MvPaceState::default();
    let mut epoch: u64 = 0;

    for (pkt_epoch, packet) in rx {
        if state.shutdown() {
            break;
        }
        if pkt_epoch < state.epoch() {
            continue;
        }
        if pkt_epoch != epoch {
            vdec.flush();
            pace = MvPaceState::default();
            epoch = pkt_epoch;
        }
        if mv_absorb_pause(&state, &cfg, epoch, &mut pace.wall_anchor) {
            if state.shutdown() { return Ok(()); }
            continue;
        }
        if let Err(e) = vdec.send_packet(&packet) {
            log::warn!(target: "mv_video", "send_packet error: {e}");
            continue;
        }
        mv_drain_video(
            &mut vdec, &mut scaler, &mut vyuv, &mut vout,
            &mut pace, vtb, &buffer, &clock,
            out_pix, out_w, out_h, hw_pix_fmt, &state, &cfg, epoch,
        );
    }
    if let Err(e) = vdec.send_eof() {
        log::warn!(target: "mv_video", "send_eof error: {e}");
    }
    mv_drain_video(
        &mut vdec, &mut scaler, &mut vyuv, &mut vout,
        &mut pace, vtb, &buffer, &clock,
        out_pix, out_w, out_h, hw_pix_fmt, &state, &cfg, epoch,
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mv_drain_video(
    vdec: &mut ffmpeg::codec::decoder::Video,
    scaler: &mut Option<(Scaler, Pixel, u32, u32)>,
    vyuv: &mut Video, vout: &mut Video,
    pace: &mut MvPaceState,
    vtb: ffmpeg::Rational,
    buffer: &MvFrameBuffer,
    clock: &Option<MvAudioClock>,
    out_pix: Pixel, out_w: u32, out_h: u32,
    hw_pix_fmt: Option<i32>,
    state: &Arc<MvPlaybackState>,
    cfg: &MvPlaybackConfig,
    epoch: u64,
) {
    while vdec.receive_frame(vyuv).is_ok() {
        if state.shutdown() { return; }
        let abs_pts = mv_pts_to_us(vyuv.timestamp().unwrap_or(0), vtb.0, vtb.1)
            .unwrap_or(0).max(0) as u64;
        // 会话首帧 PTS 共享到 state（0 哨兵表示未设置）：demux seek 时以此
        // 为偏移基准把 UI 传入的相对 seek 目标换算成流的绝对 PTS。
        // 会话级固定（seek 不重置），ts_us 保持相对进度语义。
        state.set_first_pts_us(abs_pts);
        let ts_us = abs_pts.saturating_sub(state.first_pts_us());

        if state.epoch() != epoch { continue; }

        // seek 后解码起点是目标点之前的关键帧：早于目标的帧直接丢弃
        // （不缩放、不推送、不 pacing），否则用户点了进度条却从关键帧重新播放。
        // 第一帧到达目标点后立即清零 seek_target：残留目标会让下一次 seek 的
        // 丢帧基准错误（表现为"seek 只生效一次"——第二次拖进度条画面不动）。
        if let Some(target) = state.seek_target_us() {
            if ts_us < target { continue; }
            state.clear_seek_target();
        }

        // pacing（提前到缩放之前：被丢弃的晚帧不再浪费 HW 传输和 scale 开销）
        if mv_pace_frame(pace, ts_us, clock, state, cfg, epoch) { continue; }

        // HW → SW transfer（如果硬解）
        let raw_fmt = unsafe { (*vyuv.as_ptr()).format };
        let mut sw_frame = Video::empty();
        let src: &Video = match hw_pix_fmt {
            Some(want) if raw_fmt == want => unsafe {
                let r = av_hwframe_transfer_data(sw_frame.as_mut_ptr(), vyuv.as_ptr(), 0);
                if r < 0 {
                    log::warn!(target: "mv_video", "hw transfer failed (code {r})");
                    continue;
                }
                &sw_frame
            },
            _ => &*vyuv,
        };

        let src_fmt = src.format();
        let src_w = src.width();
        let src_h = src.height();
        let need_init = match scaler {
            Some((_, f, w, h)) => *f != src_fmt || *w != src_w || *h != src_h,
            None => true,
        };
        if need_init {
            match Scaler::get(src_fmt, src_w, src_h, out_pix, out_w, out_h, Flags::BILINEAR) {
                Ok(s) => *scaler = Some((s, src_fmt, src_w, src_h)),
                Err(e) => { log::warn!(target: "mv_video", "scaler init error: {e}"); continue; }
            }
        }
        let s = match scaler.as_mut() {
            Some((s, _, _, _)) => s, None => continue,
        };
        if let Err(e) = s.run(src, vout) {
            log::warn!(target: "mv_video", "scaling error: {e}"); continue;
        }

        // Pack rows（32 对齐 → 紧凑 out_w*4 stride）
        let Some(pixels) = mv_pack_rows(
            vout.data(0), vout.stride(0), out_w as usize * 4, out_h as usize,
        ) else { continue };

        buffer.push(std::sync::Arc::new(MvFrame {
            data: pixels.into(),
            width: out_w, height: out_h, ts_us,
        }));
    }
}

fn mv_pack_rows(plane: &[u8], stride: usize, row_bytes: usize, height: usize) -> Option<Vec<u8>> {
    if stride < row_bytes || plane.len() < stride * height { return None; }
    if stride == row_bytes { return Some(plane[..row_bytes * height].to_vec()); }
    let mut packed = Vec::with_capacity(row_bytes * height);
    for row in plane.chunks_exact(stride).take(height) {
        packed.extend_from_slice(&row[..row_bytes]);
    }
    Some(packed)
}

#[derive(Default)]
struct MvPaceState {
    wall_anchor: Option<(Instant, u64)>,
}

/// 阻塞直到播放暂停结束，修正 wall-clock anchor。返回 true 表示调用方应放弃该帧。
fn mv_absorb_pause(state: &Arc<MvPlaybackState>, cfg: &MvPlaybackConfig,
                   epoch: u64, anchor: &mut Option<(Instant, u64)>) -> bool {
    if !state.paused() { return false; }
    let paused_at = Instant::now();
    let mut abort = false;
    while state.paused() {
        if state.shutdown() || state.epoch() != epoch { abort = true; break; }
        std::thread::sleep(cfg.pace_slice);
    }
    if let Some((start, _)) = anchor.as_mut() {
        *start += paused_at.elapsed();
    }
    abort
}

/// 帧 pacing：音频时钟优先，无音频时钟时回退 wall-clock。返回 true 表示帧应丢弃。
fn mv_pace_frame(pace: &mut MvPaceState, ts_us: u64, clock: &Option<MvAudioClock>,
                 state: &Arc<MvPlaybackState>, cfg: &MvPlaybackConfig,
                 epoch: u64) -> bool {
    loop {
        if state.shutdown() || state.epoch() != epoch { return true; }
        if mv_absorb_pause(state, cfg, epoch, &mut pace.wall_anchor) { return true; }

        // 缓冲门控：前方可用数据不足 → 保留当前帧原地等待（不丢帧），
        // 缓冲到 resume 阈值才继续出画；音频回调同步静音不消费，音画一起停。
        if !mv_gate_disabled(cfg, state) {
            let ahead = state.buffered_us().saturating_sub(ts_us);
            if state.buffering() {
                if ahead >= cfg.buffer_resume_us {
                    state.set_buffering(false);
                } else {
                    std::thread::sleep(cfg.pace_slice);
                    continue;
                }
            } else if ahead < cfg.buffer_enter_us {
                state.set_buffering(true);
                continue;
            }
        } else if state.buffering() {
            // 已读到流尾/配置关闭：清掉残留的缓冲态，避免 UI 一直转圈
            state.set_buffering(false);
        }

        // 音频时钟：音画同步的权威来源（暂停时时钟冻结，此处随之停住）
        if let Some(clk) = clock {
            if let Some(now) = clk.now_rel_us() {
                // 晚于 PTS 超过 late_drop_us：丢帧追时钟（不解码显示，直接放弃）
                if (ts_us as i64) + cfg.late_drop_us < now as i64 { return true; }
                if now >= ts_us { return false; }
                // 音频时钟是权威：慢于本帧就等，绝不因为等待过久而改走 wall-clock。
                // 旧实现等待超过 clock_wait_cap 就重锚 wall-clock 直接出画，
                // 音频一旦短暂断流（underrun/网络抖动），视频就会一路抢跑，
                // 音频恢复后永久错位——这正是"播着播着突然音画不同步"的根因。
                // 只有时钟整体失效（EOF / seek flush / 无音频）才回退 wall-clock。
                let wait = ts_us - now;
                std::thread::sleep(Duration::from_micros(wait).min(cfg.pace_slice));
                continue;
            }
        }

        // wall-clock 回退
        let (start_inst, first_pts) = match pace.wall_anchor {
            Some(a) => a,
            None => {
                pace.wall_anchor = Some((Instant::now(), ts_us));
                return false;
            }
        };
        let target = Duration::from_micros(ts_us.saturating_sub(first_pts));
        let elapsed = start_inst.elapsed();
        if elapsed >= target { return false; }
        let remaining = target.saturating_sub(elapsed);
        std::thread::sleep(remaining.min(cfg.pace_slice));
    }
}

/// 缓冲门控是否应关闭：已读完整个流，或"读入水位"已接近总时长（尾部无需再等 5 秒）。
fn mv_gate_disabled(cfg: &MvPlaybackConfig, state: &Arc<MvPlaybackState>) -> bool {
    if cfg.buffer_enter_us == 0 || state.eof_read() {
        return true;
    }
    let dur = state.duration_us();
    dur > 0 && state.buffered_us().saturating_add(cfg.buffer_tail_us) >= dur
}
