//! MV 抽屉内搜索 handler。
//! 接口：/cloudsearch?type=1004（MV 搜索）+ /search/suggest（建议）。
//! 状态：当前关键词、分页 offset、是否还有更多。

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;

use crate::AppWindow;
use crate::handlers::Ctx;

/// 搜索状态
struct MvSearchState {
    keyword: String,
    offset: i64,
    has_more: bool,
    loading: bool,
}

static SEARCH_STATE: OnceLock<Mutex<MvSearchState>> = OnceLock::new();
fn state() -> &'static Mutex<MvSearchState> {
    SEARCH_STATE.get_or_init(|| Mutex::new(MvSearchState {
        keyword: String::new(),
        offset: 0,
        has_more: true,
        loading: false,
    }))
}

// 搜索建议防抖 Timer（slint::Timer 非 Send，用 thread_local）。
thread_local! {
    static SUGGEST_TIMER: std::cell::RefCell<slint::Timer> = std::cell::RefCell::new(slint::Timer::default());
}
/// 搜索建议序号：输入变化时递增，用于丢弃过期响应。
static SUGGEST_SEQ: std::sync::OnceLock<std::sync::Arc<AtomicU64>> = std::sync::OnceLock::new();
fn suggest_seq() -> &'static std::sync::Arc<AtomicU64> {
    SUGGEST_SEQ.get_or_init(|| std::sync::Arc::new(AtomicU64::new(0)))
}

/// 解析搜索接口返回的 MV 列表为 MvQueueEntry。
fn parse_mv_results(raw: &serde_json::Value) -> Vec<crate::MvQueueEntry> {
    let mvs = raw.pointer("/result/mvs").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    mvs.into_iter().map(|m| {
        let id = m.get("id").and_then(|v| v.as_i64()).unwrap_or(0);
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("MV").to_string();
        let artists: Vec<String> = m.get("artists")
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .collect())
            .unwrap_or_default();
        let artist = artists.join(" / ");
        crate::MvQueueEntry {
            id: id as i32,
            name: name.into(),
            artist: artist.into(),
            duration: "--:--".into(),
            playing: false,
            cover: slint::Image::default(),
            from: slint::Color::from_argb_u8(255, 51, 51, 51),
            to: slint::Color::from_argb_u8(255, 34, 34, 34),
        }
    }).collect()
}

/// 执行 MV 搜索（重置 offset=0）。
pub fn search(ctx: &Ctx, keyword: &str) {
    let kw = keyword.trim().to_string();
    if kw.is_empty() { return; }

    {
        let mut s = state().lock().unwrap();
        s.keyword = kw.clone();
        s.offset = 0;
        s.has_more = true;
        s.loading = true;
    }
    let ui = ctx.ui.clone();
    if let Some(u) = ui.upgrade() { u.set_mv_search_loading(true); }

    let client = ctx.client.clone();
    let rt = ctx.rt.clone();
    let ui2 = ui.clone();
    rt.spawn(async move {
        let raw = match crate::network::service::search(&client, &kw, 1004, 30, 0).await {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        };

        // raw (serde_json::Value) 是 Send 的，传给 event loop 闭包，
        // 在闭包内 parse 生成含 Image 的 MvQueueEntry（Image 不 Send，不能跨线程）
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui2.upgrade() else { return };
            let entries = parse_mv_results(&raw);
            let has_more = raw.pointer("/result/hasMore").and_then(|v| v.as_bool()).unwrap_or(false);
            let count = entries.len();
            let model: ModelRc<crate::MvQueueEntry> = Rc::new(VecModel::from(entries)).into();
            u.set_mv_search_results(model);
            u.set_mv_search_loading(false);
            {
                let mut s = state().lock().unwrap();
                s.offset = count as i64;
                s.has_more = has_more;
                s.loading = false;
            }
            // 后台回填封面（cover_url 从 raw 提取）
            let cover_urls: Vec<String> = raw.pointer("/result/mvs")
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter()
                    .map(|m| m.get("cover").and_then(|v| v.as_str()).unwrap_or("").to_string())
                    .collect())
                .unwrap_or_default();
            spawn_fill_covers(&cover_urls, u.as_weak());
        });
    });
}

/// 加载更多搜索结果。
pub fn load_more(ctx: &Ctx) {
    let (keyword, offset, has_more, loading) = {
        let s = state().lock().unwrap();
        (s.keyword.clone(), s.offset, s.has_more, s.loading)
    };
    if keyword.is_empty() || !has_more || loading { return; }

    state().lock().unwrap().loading = true;

    let client = ctx.client.clone();
    let rt = ctx.rt.clone();
    let ui = ctx.ui.clone();
    rt.spawn(async move {
        let raw = match crate::network::service::search(&client, &keyword, 1004, 30, offset).await {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        };

        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui.upgrade() else { return };
            let entries = parse_mv_results(&raw);
            let has_more = raw.pointer("/result/hasMore").and_then(|v| v.as_bool()).unwrap_or(false);
            let count = entries.len();
            let model = u.get_mv_search_results();
            // ModelRc 的 as_any 转发到底层 model，因此 downcast 目标是 VecModel 本身
            // （写成 Rc<VecModel<..>> 永远失败 → 追加静默不生效，搜索分页失效）
            if let Some(vm) = model.as_any().downcast_ref::<slint::VecModel<crate::MvQueueEntry>>() {
                for e in entries { vm.push(e); }
            }
            {
                let mut s = state().lock().unwrap();
                s.offset += count as i64;
                s.has_more = has_more;
                s.loading = false;
            }
            let cover_urls: Vec<String> = raw.pointer("/result/mvs")
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter()
                    .map(|m| m.get("cover").and_then(|v| v.as_str()).unwrap_or("").to_string())
                    .collect())
                .unwrap_or_default();
            spawn_fill_covers(&cover_urls, u.as_weak());
        });
    });
}

/// 搜索建议（下拉联想）。带 300ms 防抖 + 序号校验丢弃过期响应。
pub fn suggest(ctx: &Ctx, keyword: &str) {
    let kw = keyword.trim().to_string();
    if kw.is_empty() {
        if let Some(u) = ctx.ui.upgrade() {
            u.set_mv_search_suggestions(Rc::new(VecModel::from(Vec::<slint::SharedString>::new())).into());
        }
        return;
    }

    // 序号递增：输入再次变化时旧请求的过期响应将被丢弃
    let seq = suggest_seq().fetch_add(1, Ordering::Relaxed) + 1;
    let seq_check = suggest_seq().clone();
    let client = ctx.client.clone();
    let rt = ctx.rt.clone();
    let ui = ctx.ui.clone();

    // 300ms 防抖：重复 start 替换回调并重新计时，旧回调被丢弃
    SUGGEST_TIMER.with(|t| {
        t.borrow().start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(300),
            move || {
                let kw2 = kw.clone();
                let client2 = client.clone();
                let weak3 = ui.clone();
                let rt3 = rt.clone();
                let seq_cur = seq;
                let seq_check2 = seq_check.clone();
                rt3.spawn(async move {
                    let raw = match crate::network::service::search_suggest(&client2, &kw2, 8).await {
                        Ok(v) => v,
                        Err(_) => serde_json::Value::Null,
                    };
                    // 从 result.mvs / songs / albums 里提取名称
                    let mut names: Vec<String> = Vec::new();
                    if let Some(arr) = raw.pointer("/result/mvs").and_then(|a| a.as_array()) {
                        for m in arr {
                            if let Some(n) = m.get("name").and_then(|v| v.as_str()) {
                                names.push(n.to_string());
                            }
                        }
                    }
                    if let Some(arr) = raw.pointer("/result/songs").and_then(|a| a.as_array()) {
                        for s in arr {
                            if let Some(n) = s.get("name").and_then(|v| v.as_str()) {
                                if !names.iter().any(|x| x == n) { names.push(n.to_string()); }
                            }
                        }
                    }
                    names.truncate(8);
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(u) = weak3.upgrade() else { return };
                        // 期间输入已变化 → 丢弃过期建议
                        if seq_cur != seq_check2.load(Ordering::Relaxed) { return; }
                        let model: ModelRc<slint::SharedString> = Rc::new(VecModel::from(
                            names.into_iter().map(slint::SharedString::from).collect::<Vec<_>>()
                        )).into();
                        u.set_mv_search_suggestions(model);
                    });
                });
            },
        );
    });
}

/// 点击搜索结果：把该 MV 插入到正在播放队列的下一位（“下一首播放”语义）。
/// 不替换、不清空原队列——用户正在听的队列必须保留；队列中已存在该 MV 时直接定位。
pub fn result_click(ctx: &Ctx, id: i32) {
    let mv_id = id as i64;

    // 从搜索结果里取点击条目的元数据（封面 URL 一并带上，抽屉列表直接有图）
    let (name, artist, cover_url, duration) = {
        let u = ctx.ui.upgrade().unwrap();
        let model = u.get_mv_search_results();
        let row = (0..model.row_count())
            .find(|i| model.row_data(*i).map(|e| e.id == id).unwrap_or(false))
            .and_then(|i| model.row_data(i));
        match row {
            Some(e) => (e.name.to_string(), e.artist.to_string(), String::new(), e.duration.to_string()),
            None => ("MV".to_string(), String::new(), String::new(), String::new()),
        }
    };

    let item = crate::handlers::mv::queue::MvQueueItem {
        id: mv_id,
        name: name.clone(),
        artist: artist.clone(),
        cover_url,
        duration,
        from: 0xFF333333,
        to: 0xFF222222,
    };
    if let Ok(mut q) = crate::handlers::mv::queue::queue().lock() {
        q.insert_next(item);
    }
    if let Some(u) = ctx.ui.upgrade() {
        crate::handlers::mv::queue::sync_ui(&u);
    }

    // 清除搜索状态，打开播放器
    let u = ctx.ui.upgrade().unwrap();
    u.set_mv_search_active(false);
    u.set_mv_search_text("".into());
    crate::handlers::mv::player::open_mv_player(ctx, mv_id, name, artist);
}

/// 后台异步加载搜索结果封面。
fn spawn_fill_covers(cover_urls: &[String], ui: slint::Weak<AppWindow>) {
    let page_urls: Vec<(String, crate::persist::cache::CoverType)> = cover_urls.iter()
        .filter(|u| !u.is_empty())
        .map(|u| (u.clone(), crate::persist::cache::CoverType::Mv))
        .collect();
    if page_urls.is_empty() { return; }

    let urls = cover_urls.to_vec();
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(h) => h,
        Err(_) => return,
    };

    rt.spawn(async move {
        let covers = crate::load::fetch_covers(&page_urls).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui.upgrade() else { return };
            let model = u.get_mv_search_results();
            for (idx, url) in urls.iter().enumerate() {
                if url.is_empty() { continue; }
                let Some(buf) = covers.get(url.as_str()).and_then(|x| x.as_ref()) else { continue };
                let Some(mut row) = model.row_data(idx) else { continue };
                row.cover = slint::Image::from_rgba8(buf.clone());
                model.set_row_data(idx, row);
            }
        });
    });
}

/// 注册 MV 搜索相关回调。
pub fn setup(ctx: &Ctx) {
    let ui = ctx.ui.clone();
    let client = ctx.client.clone();
    let rt = ctx.rt.clone();
    let toast = ctx.toast.clone();
    let app = ctx.app.clone_strong();

    let mk_ctx = || Ctx {
        app: app.clone_strong(),
        ui: ui.clone(),
        shared: ctx.shared.clone(),
        toast: toast.clone(),
        rt: rt.clone(),
        client: client.clone(),
    };

    ctx.app.clone_strong().on_mv_search({
        let c = mk_ctx();
        move |kw| search(&c, &kw)
    });

    ctx.app.clone_strong().on_mv_search_suggest({
        let c = mk_ctx();
        move |kw| suggest(&c, &kw)
    });

    ctx.app.clone_strong().on_mv_search_more({
        let c = mk_ctx();
        move || load_more(&c)
    });

    ctx.app.clone_strong().on_mv_search_result_click({
        let c = mk_ctx();
        move |id| result_click(&c, id)
    });
}
