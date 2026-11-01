//! web 搜索业务：标题栏搜索胶囊（输入联想下拉）+ 搜索页结果 + 播放/切页。
//!
//! 归属回调（appwindow.slint root，唯一接线方，勿与其他模块重复）：
//!   search-text-changed(string) —— 输入变化时（titlebar 内 ScrollableTextInput edited 触发）
//!   search-clicked()            —— 搜索按钮 / Enter / 建议选中（titlebar 与 SearchPage 均触发）
//!
//! 数据通道：crate::web::gd::search / gd::search_multi（netease 主通道；gdmusic 无 suggest
//! 端点 → 联想用轻量 search count=5 的结果名填充 suggest-items）。
//!
//! 关键约束：
//! - 搜索页结果填 search-results（[Song] 模型，crate::Song）；行内播放走 root.play-song(id)，
//!   歌曲 id 必须经 gd::register_id 登记（UI Song.id=i32 ↔ gd 字符串 id）。
//! - 空查询不发请求；结果为空时置 loading=false 并 toast（crate::web::show_toast）。
//! - 联想面板：suggest-items / suggest-open / suggest-index 属性驱动（titlebar 已接线
//!   suggest-move/confirm/close 键盘逻辑，只需填充模型与开关）。
//! - 防抖：输入变化 300ms 后才发联想请求（slint::Timer::single_shot 重置实现），避免每次按键打接口。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::web::gd;
use crate::web::show_toast;

pub fn setup(ui: &crate::AppWindow) {
    // 防抖 Timer：SingleShot 重复 start 会重置计时并替换回调；Rc 由 ui 闭包持有，
    // AppWindow 存活期间 Timer 不会 drop（slint Timer drop 即停）。
    let suggest_timer = Rc::new(RefCell::new(slint::Timer::default()));

    // ---- 输入变化：300ms 防抖后用 gd::search(q,5,1) 的歌名做联想下拉 ----
    let timer_cb = suggest_timer.clone();
    let ui_cb = ui.as_weak();
    ui.on_search_text_changed(move |kw| {
        let kw = kw.trim().to_string();
        if kw.is_empty() {
            // 清空输入：停掉待发的防抖请求，关闭联想面板
            timer_cb.borrow().stop();
            if let Some(u) = ui_cb.upgrade() {
                u.set_suggest_open(false);
                u.set_suggest_index(-1);
                u.set_suggest_items(ModelRc::default());
            }
            return;
        }
        let ui_t = ui_cb.clone();
        timer_cb.borrow().start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(300),
            move || {
                let kw = kw.clone();
                let ui_a = ui_t.clone();
                // wasm 单线程：spawn_local 回调回到同一线程，完成后直接 set_* 即可
                wasm_bindgen_futures::spawn_local(async move {
                    let songs = gd::search(&kw, 5, 1).await;
                    let Some(u) = ui_a.upgrade() else { return };
                    if songs.is_empty() {
                        u.set_suggest_open(false);
                        u.set_suggest_items(ModelRc::default());
                        return;
                    }
                    // gdmusic 无 suggest 端点：联想项 = "歌名 - 歌手"，kind 固定 song
                    let items: Vec<crate::SuggestItem> = songs
                        .iter()
                        .map(|s| crate::SuggestItem {
                            keyword: format!("{} - {}", s.name, s.artist).into(),
                            kind: SharedString::from("song"),
                        })
                        .collect();
                    u.set_suggest_items(Rc::new(VecModel::from(items)).into());
                    u.set_suggest_index(-1);
                    u.set_suggest_open(true);
                });
            },
        );
    });

    // ---- 执行搜索：标题栏按钮 / Enter / 联想选中（.slint 已先置 suggest-open=false 并
    //      导航 current-nav=1）→ loading=true → netease 搜索 50 条 → 填 search-results ----
    let ui_c = ui.as_weak();
    ui.on_search_clicked(move || {
        let Some(u) = ui_c.upgrade() else { return };
        let q = u.get_search_text().trim().to_string();
        if q.is_empty() {
            return; // 空词不发请求（gd::search 内部也兜底返回空）
        }
        // 与 .slint 接线双保险：执行时收起联想面板、复位选中索引
        u.set_suggest_open(false);
        u.set_suggest_index(-1);
        u.set_loading(true);
        let q = q.clone();
        let ui_s = ui_c.clone();
        wasm_bindgen_futures::spawn_local(async move {
            // netease 主通道；空结果再走 joox/tidal 多源兜底
            let mut songs = gd::search(&q, 50, 1).await;
            if songs.is_empty() {
                songs = gd::search_multi(&q, 50, 1).await;
            }
            let Some(u) = ui_s.upgrade() else { return };
            u.set_loading(false);
            if songs.is_empty() {
                show_toast(&u, "未找到相关歌曲", "info");
                u.set_search_results(ModelRc::default());
                return;
            }
            // 映射为 UI Song：id 经 gd::register_full 登记（play.rs 凭 UI id 重建播放），
            // 封面阶段 1b 统一空图走组件渐变兜底；duration 留空串。
            let rows: Vec<crate::Song> = songs
                .iter()
                .map(|s| {
                    let id = gd::register_full(
                        &s.source,
                        &s.id,
                        &s.name,
                        &s.artist,
                        &s.album,
                        0.0,
                    );
                    crate::Song {
                        id,
                        name: s.name.clone().into(),
                        artist: s.artist.clone().into(),
                        album: s.album.clone().into(),
                        cover: gd::cover_image(""),
                        duration: SharedString::new(),
                        duration_sec: 0.0,
                        url: SharedString::new(),
                        liked: gd::liked_contains(id),
                    }
                })
                .collect();
            u.set_search_results(Rc::new(VecModel::from(rows)).into());
            // 阶段 1b：按行序异步回填封面（pics 与上方 rows 行序一一对应；
            // search_multi 兜底路径同样取各 source 的 pic_id）。失败静默保持渐变，不 toast。
            let m = u.get_search_results();
            for (i, s) in songs.iter().enumerate().take(30) {
                let pic = s.pic_id.clone();
                if pic.is_empty() {
                    continue;
                }
                let mm = m.clone();
                gd::spawn_cover_pic(pic, move |img| {
                    if let Some(img) = img {
                        if let Some(mut row) = mm.row_data(i) {
                            row.cover = img.clone();
                            mm.set_row_data(i, row);
                        }
                    }
                });
            }
        });
    });
}
