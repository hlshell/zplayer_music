//! MvFrame → slint::Image 适配层。
//! 解码器直接输出 RGBA8 / BGRA8（sws_scale 在后台线程完成），
//! 这里直接包装成 Slint SharedPixelBuffer，UI 线程零转换。

use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;
use crate::core::mv_frame::MvFrame;

/// 只做像素拷贝（每帧唯一的大内存拷贝）。
/// SharedPixelBuffer 是 Send 的（Image 不是），可在 worker 线程完成拷贝后
/// 跨线程交给 UI 线程，由 Image::from_rgba8 零拷贝包装。
pub fn mv_frame_to_pixel_buffer(frame: &Arc<MvFrame>) -> SharedPixelBuffer<Rgba8Pixel> {
    // data 是 Arc<[u8]>，我们 clone 一份像素数据到 Slint buffer 里。
    // player-core 的 Frame.data 是 Arc<[u8]>，已经是紧凑 out_w*4 stride。
    SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &frame.data, frame.width, frame.height,
    )
}
