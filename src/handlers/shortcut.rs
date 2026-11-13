//! 应用内快捷键：键位规范化、配置持久化与动作分发。
//!
//! UI 侧（appwindow.slint 的 FocusScope）只上报原始按键（文本 + 修饰键），
//! 这里统一完成「规范化 → 与配置匹配 → 分发动作」；设置页捕获新键位复用同一套规范化。

use super::{Ctx, ToastCtx};
use crate::persist::config::SHORTCUT_IDS;
use crate::AppWindow;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// 音量步进
const VOLUME_STEP: f32 = 0.05;
/// 快进 / 快退秒数
const SEEK_SEC: f32 = 5.0;

/// 把 Slint KeyEvent 的文本 + 修饰键规范化为配置里的组合串（如 "Ctrl+Shift+Left"）。
/// 返回 None 表示当前按键不作为快捷键（只按了修饰键、输入法组合等）。
fn normalize(text: &str, ctrl: bool, alt: bool, shift: bool) -> Option<String> {
    let key = match text {
        // 控制字符（编码见 i-slint-common/key_codes.rs）
        "\u{0008}" => "Backspace".to_string(),
        "\u{0009}" => "Tab".to_string(),
        "\u{000a}" => "Enter".to_string(),
        "\u{001b}" => "Escape".to_string(),
        "\u{007f}" => "Delete".to_string(),
        "\u{0020}" => "Space".to_string(),
        // 方向 / 编辑键（Slint 私有区编码）
        "\u{F700}" => "Up".to_string(),
        "\u{F701}" => "Down".to_string(),
        "\u{F702}" => "Left".to_string(),
        "\u{F703}" => "Right".to_string(),
        "\u{F729}" => "Home".to_string(),
        "\u{F72B}" => "End".to_string(),
        "\u{F72C}" => "PageUp".to_string(),
        "\u{F72D}" => "PageDown".to_string(),
        // 修饰键自身：忽略，继续等待真正按键
        "\u{0010}" | "\u{0011}" | "\u{0012}" | "\u{0013}"
        | "\u{0014}" | "\u{0015}" | "\u{0016}" | "\u{0017}" | "\u{0018}" => return None,
        other => {
            let mut it = other.chars();
            let c = it.next()?;
            if it.next().is_some() {
                return None; // 多字符（输入法组合等）不作快捷键
            }
            c.to_uppercase().collect::<String>()
        }
    };
    if key.is_empty() {
        return None;
    }
    let mut combo = String::new();
    if ctrl {
        combo.push_str("Ctrl+");
    }
    if alt {
        combo.push_str("Alt+");
    }
    if shift {
        combo.push_str("Shift+");
    }
    combo.push_str(&key);
    Some(combo)
}

/// 把组合键列表推到 UI（顺序必须与 SHORTCUT_IDS 一致）
fn push_combos(u: &AppWindow, list: Vec<String>) {
    let items: Vec<SharedString> = list.into_iter().map(SharedString::from).collect();
    u.set_shortcut_combos(ModelRc::new(VecModel::from(items)));
}

/// 执行动作
fn dispatch(u: &AppWindow, id: &str) {
    match id {
        "play_pause" => u.invoke_play_toggle(),
        "prev" => u.invoke_prev_song(),
        "next" => u.invoke_next_song(),
        "seek_back" | "seek_forward" => {
            let dur = u.get_current_song().duration_sec;
            if dur <= 0.0 {
                return;
            }
            let delta = SEEK_SEC / dur;
            let target = if id == "seek_back" {
                u.get_progress() - delta
            } else {
                u.get_progress() + delta
            };
            u.invoke_seek(target.clamp(0.0, 1.0));
        }
        "volume_up" | "volume_down" => {
            let step = if id == "volume_up" { VOLUME_STEP } else { -VOLUME_STEP };
            let v = (u.get_volume() + step).clamp(0.0, 1.0);
            u.set_volume(v);
            u.set_muted(false);
            if v > 0.0 {
                u.set_pre_volume(v);
            }
            u.invoke_volume_changed(v);
        }
        "mute" => {
            if u.get_muted() {
                let v = u.get_pre_volume().clamp(0.0, 1.0);
                u.set_volume(v);
                u.set_muted(false);
                u.invoke_volume_changed(v);
            } else {
                u.set_pre_volume(u.get_volume());
                u.set_volume(0.0);
                u.set_muted(true);
                u.invoke_volume_changed(0.0);
            }
        }
        "home" => u.set_current_nav(0),
        "settings" => u.set_current_nav(99),
        "hide" => {
            let _ = u.window().hide();
        }
        _ => {}
    }
}

pub fn setup(ctx: &Ctx) {
    let ui = ctx.ui.clone();
    let shared = ctx.shared.clone();

    // 初始填充（配置文件里的组合键）
    if let Some(u) = ui.upgrade() {
        let list = shared.borrow().shortcut.to_list();
        push_combos(&u, list);
    }

    // 应用内按键：匹配已配置的组合键并执行动作
    {
        let ui = ui.clone();
        let shared = shared.clone();
        ctx.app.clone_strong().on_shortcut_key(move |text, ctrl, alt, shift| {
            let Some(combo) = normalize(text.as_str(), ctrl, alt, shift) else {
                return;
            };
            let Some(u) = ui.upgrade() else { return };
            let cfg = shared.borrow();
            let hit = SHORTCUT_IDS
                .iter()
                .find(|id| cfg.shortcut.get(id).eq_ignore_ascii_case(&combo))
                .map(|id| id.to_string());
            drop(cfg);
            if let Some(id) = hit {
                dispatch(&u, &id);
            }
        });
    }

    // 设置页捕获新键位：规范化 → 冲突校验 → 写入配置并持久化
    {
        let ui = ui.clone();
        let shared = shared.clone();
        ctx.app.clone_strong().on_shortcut_capture(move |idx, text, ctrl, alt, shift| {
            let Some(combo) = normalize(text.as_str(), ctrl, alt, shift) else {
                return; // 只按了修饰键，继续等待
            };
            let Some(u) = ui.upgrade() else { return };
            let i = idx as usize;
            let Some(&id) = SHORTCUT_IDS.get(i) else { return };
            let mut c = shared.borrow_mut();
            if c.shortcut.conflicting(&combo, id).is_some() {
                drop(c);
                let msg = u.global::<crate::Language>().get_shortcut_conflict_toast();
                let t = ToastCtx { weak: ui.clone() };
                t.show(msg.as_str(), "warn");
                return;
            }
            c.shortcut.set(id, &combo);
            c.save();
            let list = c.shortcut.to_list();
            drop(c);
            push_combos(&u, list);
            u.set_shortcut_capturing(-1);
            let msg = u.global::<crate::Language>().get_shortcut_updated_toast();
            let t = ToastCtx { weak: ui.clone() };
            t.show(msg.as_str(), "success");
        });
    }

    // 恢复默认
    {
        let ui = ui.clone();
        let shared = shared.clone();
        ctx.app.clone_strong().on_shortcut_reset(move || {
            let Some(u) = ui.upgrade() else { return };
            let mut c = shared.borrow_mut();
            c.shortcut = crate::persist::config::ShortcutSection::default();
            c.save();
            let list = c.shortcut.to_list();
            drop(c);
            push_combos(&u, list);
            u.set_shortcut_capturing(-1);
        });
    }
}
