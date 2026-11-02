//! MV 播放器解码器主循环（内嵌自 rs-wgpu-video-player/player-core/src/decoder/*）。
//! 把 demux → 视频解码 + 音频解码（cpal 输出）→ 帧缓冲 串起来。
//! 有音频流且输出设备可用时，视频 pacing 跟音频时钟；否则回退 wall-clock。

use crate::core::mv_config::MvPlaybackConfig;
use crate::core::mv_error::{MvPlayerError, MvResult};
use crate::core::mv_event::MvPlaybackEvent;
use crate::core::mv_frame::MvFrameBuffer;
use crate::core::mv_state::MvPlaybackState;

use ffmpeg::ffi::AV_TIME_BASE;
use ffmpeg_next as ffmpeg;
use std::sync::{mpsc, Arc};
use tokio::sync::mpsc::UnboundedSender;

/// UI → demux loop 控制通道。
#[derive(Debug)]
pub enum MvDemuxCommand {
    /// 跳到微秒（0-based 时间轴）。
    Seek(u64),
}

/// 主入口：spawn_blocking 跑完整 demux → 解码 → 推帧。
#[allow(clippy::too_many_arguments)]
pub fn mv_loop_decoder(
    input: String,
    buffer: MvFrameBuffer,
    events: UnboundedSender<MvPlaybackEvent>,
    state: Arc<MvPlaybackState>,
    cfg: MvPlaybackConfig,
    cmd_rx: mpsc::Receiver<MvDemuxCommand>,
) -> MvResult<()> {
    // HLS 探针节流：避免并行拉所有码率变体
    let mut opts = ffmpeg::Dictionary::new();
    opts.set("probesize", "500000");
    opts.set("analyzeduration", "500000");
    // 加大协议层读缓冲：网络抖动时一次多取一些，减少 av_read_frame 的等待次数
    opts.set("buffer_size", "1048576");
    let mut ictx = ffmpeg::format::input_with_dictionary(&input, opts)?;

    // 时间轴基准：容器 start_time（绝对，AV_TIME_BASE 微秒）。
    // 视频 ts 与音频时钟都用它做零点——若视频用"首帧 PTS"、音频用"首样本 PTS"，
    // 两条流首包时间不一致时会产生固定音画偏移（表现为声音早/画面滞留）。
    match unsafe { (*ictx.as_ptr()).start_time } {
        // 有效 start_time：直接锁为共同时间轴零点
        v if v != ffmpeg::ffi::AV_NOPTS_VALUE && v >= 0 => state.set_first_pts_us(v as u64),
        // 未知：留给视频线程用首帧 PTS 兜底（音频侧同样回退首样本 PTS）
        _ => {}
    }

    // 发射容器总时长
    if ictx.duration() > 0 {
        let dur_us = (ictx.duration() as i128 * 1_000_000i128 / AV_TIME_BASE as i128) as u64;
        state.set_duration_us(dur_us);
        let _ = events.send(MvPlaybackEvent::Duration(dur_us));
    }

    // 选视频流
    let vstream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(MvPlayerError::NoVideoStream)?;
    let vindex = vstream.index();
    let vparams = vstream.parameters();
    let vtb = vstream.time_base();

    // 选音频流（存在则解码输出到 cpal，同时充当视频 pacing 的权威时钟）
    let aparams = ictx
        .stream(vindex)
        .and_then(|video| ictx.streams().related(&video).best(ffmpeg::media::Type::Audio))
        .map(|s| (s.index(), s.parameters(), s.time_base()));

    // 丢弃无关流（HLS 多码率时只拉选中的一条）
    mv_discard_other_streams(&mut ictx, vindex, aparams.as_ref().map(|a| a.0));

    log::info!(target: "mv_demux", "selected video stream {vindex}, audio {}",
        aparams.as_ref().map(|a| a.0).unwrap_or(usize::MAX));

    // demux → video 解码 包队列
    let (vtx, vrx) = mpsc::sync_channel::<crate::core::mv_demux::MvTaggedPacket>(cfg.video_pkt_queue);

    // 音频时钟（预创建，可跨线程）+ 解码线程（sink 在线程内创建：cpal::Stream 非 Send）。
    // 无音频流或设备不可用时时钟保持无效，视频回退 wall-clock。
    let a_clock = crate::core::mv_audio::MvAudioClock::new();
    let audio = match aparams {
        Some((aidx, aparams_v, atb)) => {
            let (atx, arx) =
                mpsc::sync_channel::<crate::core::mv_demux::MvTaggedPacket>(cfg.audio_pkt_queue);
            let a_state = state.clone();
            let a_clock_t = a_clock.clone();
            let handle = std::thread::spawn(move || {
                if let Err(e) =
                    crate::core::mv_decoder_audio::mv_audio_decode_thread(arx, aparams_v, atb, a_clock_t, a_state)
                {
                    log::warn!(target: "mv_audio", "thread error: {e}");
                }
            });
            log::info!(target: "mv_audio", "audio pipeline enabled (stream {aidx})");
            Some((handle, atx, aidx))
        }
        None => {
            log::info!(target: "mv_audio", "no audio stream, video-only");
            None
        }
    };

    // 视频解码线程（有音频时钟则跟音频走，否则 wall-clock pacing）
    let v_state = state.clone();
    let v_buffer = buffer.clone();
    let v_cfg = cfg;
    let v_clock = if audio.is_some() { Some(a_clock) } else { None };
    let video_handle = std::thread::spawn(move || {
        if let Err(e) = crate::core::mv_decoder_video::mv_video_decode_thread(
            vrx, vparams, vtb, v_buffer, v_clock, v_state, v_cfg,
        ) {
            log::warn!(target: "mv_video", "thread error: {e}");
        }
    });

    // demux 主循环：Err = 读流中断（网络断流等，非自然播完）
    let demux_res = crate::core::mv_demux::mv_run_demux(
        &mut ictx, vindex, &vtx,
        audio.as_ref().map(|a| a.2),
        audio.as_ref().map(|a| &a.1),
        &state, &cfg, &cmd_rx,
    );

    drop(vtx);
    // demux 已读完整个流：关闭缓冲门控（否则结尾几帧会永远等不到"5 秒可用数据"）
    state.set_eof_read(true);
    // 先断开音频包通道：音频线程 drain 完缓存后退出并将时钟置为无效，
    // 视频 pacing 才能回退 wall-clock 播完"音频流比视频流短"的尾部帧。
    // 否则音频时钟冻结在末尾 → 视频线程永远等时钟 → join(video) 死锁。
    let audio = audio.map(|(a_handle, atx, aidx)| {
        drop(atx);
        (a_handle, aidx)
    });

    if let Err(e) = video_handle.join() {
        log::warn!(target: "mv_video", "thread panicked: {e:?}");
    }
    if let Some((a_handle, _)) = audio {
        if let Err(e) = a_handle.join() {
            log::warn!(target: "mv_audio", "thread panicked: {e:?}");
        }
    }

    // 主动 stop（shutdown 置位）不算"自然播完"，不发 Ended——
    // 否则用户关闭播放器会触发自动下一首。
    if !state.shutdown() {
        match demux_res {
            // 读到流末尾：自然播完 → 自动连播
            Ok(()) => { let _ = events.send(MvPlaybackEvent::Ended); }
            // 读流中断（网络断流等）：上报错误，由上层决定重连当前 MV，
            // 绝不伪装成 Ended 触发"跳到下一首"。
            Err(msg) => { let _ = events.send(MvPlaybackEvent::Error(msg)); }
        }
    }
    Ok(())
}

/// 让 ffmpeg 丢弃无关流（HLS 多码率场景避免并行下载）。
fn mv_discard_other_streams(
    ictx: &mut ffmpeg::format::context::Input,
    video: usize,
    audio: Option<usize>,
) {
    for mut stream in ictx.streams_mut() {
        let index = stream.index();
        if index == video || Some(index) == audio {
            continue;
        }
        unsafe {
            (*stream.as_mut_ptr()).discard = ffmpeg::Discard::All.into();
        }
    }
}

/// PTS → 微秒换算（带溢出保护）。
#[inline]
pub fn mv_pts_to_us(pts: i64, tb_num: i32, tb_den: i32) -> Option<i64> {
    if tb_den == 0 {
        return None;
    }
    let us = (pts as i128) * (tb_num as i128) * 1_000_000i128 / (tb_den as i128);
    Some(us as i64)
}
