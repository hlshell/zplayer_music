//! 播放列表管理回调：清空队列、移除单曲。

use super::Ctx;
use crate::Song;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

pub(crate) fn setup_queue(ctx: &Ctx) {
    // ---- 清空播放列表 / 移除单曲 ----
    let ui_weak = ctx.ui.clone();
    let toast_cb = ctx.toast.clone();
    ctx.app.clone_strong().on_clear_queue(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        u.set_queue(Rc::new(VecModel::default()).into());
        toast_cb.show("已清空播放列表", "info");
    });

    let ui_weak = ctx.ui.clone();
    let toast_cb = ctx.toast.clone();
    let last_remove = Rc::new(Cell::new(std::time::Instant::now() - Duration::from_secs(1)));
    ctx.app.clone_strong().on_remove_song(move |id| {
        if last_remove.get().elapsed() < Duration::from_millis(300) {
            return;
        }
        last_remove.set(std::time::Instant::now());
        let Some(u) = ui_weak.upgrade() else { return; };
        let target = id as u32 as i64;
        let m = u.get_queue();
        let mut removed: Option<String> = None;
        if let Some(v) = m.as_any().downcast_ref::<VecModel<Song>>() {
            for i in 0..v.row_count() {
                if let Some(s) = v.row_data(i) {
                    if s.id as u32 as i64 == target {
                        removed = Some(s.name.to_string());
                        v.remove(i);
                        break;
                    }
                }
            }
        }
        if let Some(name) = removed {
            toast_cb.show(&format!("已移除：{}", name), "info");
        }
    });
}
