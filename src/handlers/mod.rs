//! handlers：把 fn main 里几十个 `on_*` 回调按业务分组到独立文件，
//! 每个文件一个 `setup(ctx)` 入口；`fn main` 只负责初始化 + 装配各 handler。
//!
//! 共享上下文 Ctx：回调所需的 ui/shared/toast/rt/client 统一在此传递，
//! 各 handler 的 setup 从 Ctx 克隆所需句柄（Weak/Arc/Rc 均为克隆廉价）。

use crate::persist::config::AppConfig;
use crate::AppWindow;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

pub mod browse;
pub mod cache;
pub mod fav;
pub mod local;
pub mod modwin;
pub mod mv;
pub mod player;
pub mod search;
pub mod settings;
pub mod shortcut;
pub mod toplist;

thread_local! {
    /// 页面切换填充钩子（多 handler 共享）。
    /// Slint 的 `on_page_fill` 是单一回调，重复注册会覆盖前一个；
    /// 这里内部只绑定一次，向所有订阅者分发。
    static PAGE_FILL_HOOKS: RefCell<Vec<Box<dyn Fn(i32)>>> = RefCell::new(Vec::new());
    static PAGE_FILL_BOUND: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// 订阅页面切换填充事件（nav 索引），供各 handler 各自响应自己关心的页面。
pub fn on_page_fill(ctx: &Ctx, hook: impl Fn(i32) + 'static) {
    PAGE_FILL_HOOKS.with(|h| h.borrow_mut().push(Box::new(hook)));
    if !PAGE_FILL_BOUND.with(|b| b.replace(true)) {
        ctx.app.clone_strong().on_page_fill(|nav| {
            PAGE_FILL_HOOKS.with(|h| {
                for f in h.borrow().iter() {
                    f(nav);
                }
            });
        });
    }
}

/// Toast 轻提示（显示后 2.2s 自动隐藏；须在 UI 线程调用）。
/// 只持有 Weak（Send），可用 Arc 在 tokio 异步任务中共享；
/// 自动隐藏用 Timer::single_shot（无需持有 Timer 实例）。
pub struct ToastCtx {
    pub weak: slint::Weak<AppWindow>,
}
impl ToastCtx {
    pub fn show(&self, text: &str, kind: &str) {
        if let Some(u) = self.weak.upgrade() {
            u.set_toast_text(text.into());
            u.set_toast_kind(kind.into());
            u.set_toast_show(true);
            let uw = self.weak.clone();
            slint::Timer::single_shot(Duration::from_millis(2200), move || {
                if let Some(u) = uw.upgrade() {
                    u.set_toast_show(false);
                }
            });
        }
    }
}

/// 回调共享上下文。
/// `app` 为强引用（主线程，供注册 on_* 回调）；`ui` 为 Weak（回调内 clone，可进异步任务）。
pub struct Ctx {
    pub app: AppWindow,
    pub ui: slint::Weak<AppWindow>,
    pub shared: Rc<RefCell<AppConfig>>,
    pub toast: Arc<ToastCtx>,
    pub rt: tokio::runtime::Handle,
    pub client: Arc<crate::network::netease::client::NeteaseClient>,
}

impl Ctx {
    pub fn new(
        app: AppWindow,
        ui: slint::Weak<AppWindow>,
        shared: Rc<RefCell<AppConfig>>,
        toast: Arc<ToastCtx>,
        rt: tokio::runtime::Handle,
        client: Arc<crate::network::netease::client::NeteaseClient>,
    ) -> Self {
        Self { app, ui, shared, toast, rt, client }
    }
}
