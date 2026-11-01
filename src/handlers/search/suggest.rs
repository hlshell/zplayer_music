//! 搜索建议：输入防抖 300ms 后拉取 suggest 接口，填充弹出面板。
//! 面板的开关 / 键盘导航 / 填词搜索均在 .slint 完成，这里只负责数据。

use super::Ctx;
use crate::core::fetch;
use slint::{ComponentHandle, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub(crate) fn setup_suggest(ctx: &Ctx) {
    let ui_weak = ctx.ui.clone();
    let client_cb = ctx.client.clone();
    let rt_handle = ctx.rt.clone();
    let suggest_timer = Rc::new(std::cell::RefCell::new(slint::Timer::default()));
    let suggest_seq = std::sync::Arc::new(AtomicU64::new(0));
    let suggest_timer_cb = suggest_timer.clone();
    let suggest_seq_cb = suggest_seq.clone();
    // Timer 需持续存活（防抖用 SingleShot 重复 start），drop 即停止 → leak 持有
    std::mem::forget(suggest_timer);
    ctx.app.clone_strong().on_search_text_changed(move |kw| {
        let Some(u) = ui_weak.clone().upgrade() else { return; };
        let kw = kw.trim().to_string();
        if kw.is_empty() {
            u.set_suggest_open(false);
            u.set_suggest_index(-1);
            u.set_suggest_items(Rc::new(VecModel::from(Vec::<crate::SuggestItem>::new())).into());
            return;
        }
        // 序号递增：一旦输入再次变化，旧请求的过期响应将被丢弃
        let seq = suggest_seq_cb.load(Ordering::Relaxed) + 1;
        suggest_seq_cb.store(seq, Ordering::Relaxed);
        let seq_cur = seq;
        let seq_check = suggest_seq_cb.clone();
        let client = client_cb.clone();
        let weak2 = ui_weak.clone();
        let rt_handle2 = rt_handle.clone();
        // 300ms 防抖：重复 start 会替换回调并重新计时，旧的回调被丢弃
        suggest_timer_cb.borrow().start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(300),
            move || {
                let kw2 = kw.clone();
                let client2 = client.clone();
                let weak3 = weak2.clone();
                let rt3 = rt_handle2.clone();
                let seq_check2 = seq_check.clone();
                rt3.spawn(async move {
                    let items: Vec<(String, String)> =
                        match crate::network::service::search_suggest(&client2, &kw2, 8).await {
                            Ok(v) => fetch::parse_suggest_items(&v),
                            Err(_) => Vec::new(),
                        };
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = weak3.upgrade() {
                            // 期间输入已变化 → 丢弃过期建议
                            if seq_cur != seq_check2.load(Ordering::Relaxed) { return; }
                            let list: Vec<crate::SuggestItem> = items
                                .into_iter()
                                .map(|(k, kind)| crate::SuggestItem { keyword: k.into(), kind: kind.into() })
                                .collect();
                            u.set_suggest_items(Rc::new(VecModel::from(list)).into());
                            u.set_suggest_index(-1);
                        }
                    });
                });
            },
        );
    });
}
