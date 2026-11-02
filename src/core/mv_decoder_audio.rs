//! 音频解码线程（内嵌自 player-core/src/decoder/audio.rs，精简版）。
//! AAC(等) 解码 → swresample 重采样为设备格式的 f32 交错 PCM → 推入 MvAudioSink。
//! 包队列由 demux 填入（sync_channel 背压）；seek 靠 epoch 检测后 flush 解码器 + sink。
//! 暂停无需显式处理：demux 暂停时不发新包，本线程阻塞在 recv 上；cpal 回调静音且不消费环。

use crate::core::mv_audio::{MvAudioClock, MvAudioSink};
use crate::core::mv_decoder::mv_pts_to_us;
use crate::core::mv_demux::MvTaggedPacket;
use crate::core::mv_error::MvResult;
use crate::core::mv_state::MvPlaybackState;

use ffmpeg::codec::context::Context;
use ffmpeg::format::Sample;
use ffmpeg::software::resampling::Context as Resampler;
use ffmpeg::util::frame::Audio;
use ffmpeg_next as ffmpeg;

use std::sync::{mpsc, Arc};

pub fn mv_audio_decode_thread(
    rx: mpsc::Receiver<MvTaggedPacket>,
    aparams: ffmpeg::codec::Parameters,
    atb: ffmpeg::Rational,
    clock: MvAudioClock,
    state: Arc<MvPlaybackState>,
) -> MvResult<()> {
    // cpal::Stream 非 Send，sink 必须在本线程内创建。
    // 设备不可用时排空包队列（避免 demux 的 send 阻塞拖住视频），时钟保持无效 → 视频走 wall-clock。
    let sink = match MvAudioSink::new(state.clone(), &clock) {
        Some(s) => s,
        None => {
            log::warn!(target: "mv_audio", "no output device, audio disabled");
            while rx.recv().is_ok() {
                if state.shutdown() { break; }
            }
            clock.invalidate();
            return Ok(());
        }
    };
    let ctx = Context::from_parameters(aparams)?;
    let codec = ffmpeg::decoder::find(ctx.id()).ok_or(crate::core::mv_error::MvPlayerError::DecoderNotFound)?;
    let mut adec = ctx.decoder().open_as(codec).and_then(|o| o.audio())?;

    let out_rate = sink.sample_rate();
    let out_layout = if sink.channels() == 1 {
        ffmpeg::channel_layout::ChannelLayout::MONO
    } else {
        ffmpeg::channel_layout::ChannelLayout::STEREO
    };
    let out_fmt = Sample::F32(ffmpeg::format::sample::Type::Packed);

    // (resampler, in_fmt, in_layout, in_rate)——输入参数变化时重建
    let mut resampler: Option<(Resampler, Sample, ffmpeg::channel_layout::ChannelLayout, u32)> = None;
    let mut frame = Audio::empty();
    let mut epoch = state.epoch();
    // 帧 PTS 缺失时的续推时间（容器绝对 us）
    let mut next_abs_us: Option<u64> = None;

    for (pkt_epoch, packet) in rx {
        if state.shutdown() {
            break;
        }
        if pkt_epoch < state.epoch() {
            continue; // seek 前的过期包
        }
        if pkt_epoch != epoch {
            // seek：清解码器 + 重采样器 + 采样环/时钟（时钟短暂失效，视频 pacing 暂回退 wall-clock）
            adec.flush();
            sink.flush();
            // 重采样器内部带滤波延迟与历史样本，seek 后必须重建，否则接缝处出杂音
            resampler = None;
            epoch = pkt_epoch;
            next_abs_us = None;
        }
        if adec.send_packet(&packet).is_err() {
            continue;
        }
        while adec.receive_frame(&mut frame).is_ok() {
            if state.shutdown() {
                clock.invalidate();
                return Ok(());
            }
            let in_fmt = frame.format();
            let in_layout = frame.channel_layout();
            let in_rate = frame.rate();
            let need_rebuild = match &resampler {
                Some((_, f, l, r)) => *f != in_fmt || *l != in_layout || *r != in_rate,
                None => true,
            };
            if need_rebuild {
                match Resampler::get(in_fmt, in_layout, in_rate, out_fmt, out_layout, out_rate) {
                    Ok(r) => resampler = Some((r, in_fmt, in_layout, in_rate)),
                    Err(e) => {
                        log::warn!(target: "mv_audio", "resampler init: {e}");
                        continue;
                    }
                }
            }
            let rs = &mut resampler.as_mut().unwrap().0;
            // ffmpeg-next 的 Resampler::run 只在输出为空帧时按 input.samples() 分配容量，
            // 且 swr_convert_frame 复用已分配帧时以 nb_samples 为上限——上采样（44.1k→48k）
            // 会被逐帧截成 1:1，导致播放快 8.8%。这里按换算比例 + 余量预分配输出帧。
            let est = (frame.samples() as u64 * out_rate as u64 / in_rate.max(1) as u64) as usize + 256;
            let mut out = Audio::empty();
            unsafe { out.alloc(out_fmt, est, out_layout) };
            if rs.run(&frame, &mut out).is_err() {
                continue;
            }
            let n = out.samples();
            if n == 0 {
                continue;
            }
            let ch = out.channels() as usize;
            if ch == 0 || out.data(0).len() < n * ch * 4 {
                continue;
            }
            let ptr = out.data(0).as_ptr() as *const f32;
            let samples = unsafe { std::slice::from_raw_parts(ptr, n * ch) };
            let dur_us = n as u64 * 1_000_000 / out_rate.max(1) as u64;
            let abs_us = frame
                .timestamp()
                .and_then(|ts| mv_pts_to_us(ts, atb.0, atb.1))
                .filter(|v| *v >= 0)
                .map(|v| v as u64)
                .unwrap_or_else(|| next_abs_us.unwrap_or(0));
            next_abs_us = Some(abs_us + dur_us);
            // seek 后音频从目标点之前的最近关键帧开始：丢掉目标点之前的采样，
            // 保证音频与视频都从用户点击的位置起播（否则音频会从旧位置响）。
            if let Some(target_rel) = state.seek_target_us() {
                let base = if state.first_pts_ready() { state.first_pts_us() } else { 0 };
                if abs_us.saturating_add(dur_us) <= base.saturating_add(target_rel) {
                    continue;
                }
            }
            if !sink.push(samples, abs_us, abs_us + dur_us, &state, epoch) {
                clock.invalidate();
                return Ok(());
            }
        }
    }
    // EOF（rx 断开）或 shutdown 退出：时钟失效，视频 pacing 回退 wall-clock 播完尾部。
    clock.invalidate();
    Ok(())
}
