//! 纯逻辑：根据可用逻辑尺寸判定移动端布局形态，便于单元测试。
//! 规则见 spec §4：最短边 < 600dp 为手机（宽>高为横屏，否则竖屏）；
//! 最短边 >= 600dp 为平板。内容可用宽 >= 840dp 时平板启用双栏。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    PhonePortrait,
    PhoneLandscape,
    Tablet,
}

pub fn classify(width: f32, height: f32) -> LayoutKind {
    let shortest = width.min(height);
    if shortest >= 600.0 {
        LayoutKind::Tablet
    } else if width > height {
        LayoutKind::PhoneLandscape
    } else {
        LayoutKind::PhonePortrait
    }
}

/// 平板内容区是否采用双栏（spec §4：内容可用宽度 >= 840dp）。
pub fn should_use_two_column(content_width: f32) -> bool {
    content_width >= 840.0
}
