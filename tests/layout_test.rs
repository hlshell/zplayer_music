use zplayer_music::ui::layout::{classify, LayoutKind};

#[test]
fn phone_portrait_under_600_short_side() {
    // 390 x 844 (iPhone 14 逻辑尺寸)
    assert_eq!(classify(390.0, 844.0), LayoutKind::PhonePortrait);
}

#[test]
fn phone_landscape_short_side_under_600() {
    assert_eq!(classify(844.0, 390.0), LayoutKind::PhoneLandscape);
}

#[test]
fn tablet_portrait_short_side_at_least_600() {
    // 768 x 1024 (iPad)
    assert_eq!(classify(768.0, 1024.0), LayoutKind::Tablet);
}

#[test]
fn tablet_landscape_is_tablet() {
    assert_eq!(classify(1024.0, 768.0), LayoutKind::Tablet);
}

#[test]
fn tablet_two_column_threshold_840_content_width() {
    use zplayer_music::ui::layout::should_use_two_column;
    assert!(!should_use_two_column(839.0));
    assert!(should_use_two_column(840.0));
    assert!(should_use_two_column(1024.0));
}

#[test]
fn square_small_is_phone_landscape_when_width_equals_height() {
    // 高>=宽时算竖屏；这里 600x600 最短边达到600 → 平板
    assert_eq!(classify(600.0, 600.0), LayoutKind::Tablet);
}
