//! MvFrame → slint::Image 适配层。
//! 解码器直接输出 RGBA8 / BGRA8（sws_scale 在后台线程完成），
//! 这里直接包装成 Slint SharedPixelBuffer，UI 线程零转换。

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::sync::Arc;
use crate::core::mv_frame::MvFrame;

pub fn mv_frame_to_image(frame: &Arc<MvFrame>) -> Image {
    // data 是 Arc<[u8]>，我们 clone 一份像素数据到 Slint buffer 里。
    // player-core 的 Frame.data 是 Arc<[u8]>，已经是紧凑 out_w*4 stride。
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &frame.data, frame.width, frame.height,
    );
    Image::from_rgba8(buf)
}
