//! 歌单页回调：分类切换（按分类缓存，切回不重拉）+ 触底分页。
//!
//! 防过度请求设计（2026-10-25 修复）：
//! - 每个分类的数据按分类缓存；切回已加载的分类直接恢复 UI，不再发网络请求。
//! - 点击当前已激活分类直接跳过（不重复刷新）。
//! - 切换分类时显式复位 need-more / loading-more / no-more，避免旧分类残留的
//!   自动补页信号在新分类上继续触发请求。
//! - UI 侧自动补页设数量上限（见 playlists.slint，内容不足一屏最多补到 60 条），
//!   避免内容撑不满视口时无限串行拉页。

use super::Ctx;
use crate::AppWindow;
use crate::core::song_util::fmt_plays_cat;
use slint::{ComponentHandle, Model, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 单分类行元组：id,name,plays,url,from,to
type Row = (i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color);

pub(crate) fn setup_playlists(ctx: &Ctx) {
    // live：当前分类的加载状态（驱动触底分页）
    let live: Arc<Mutex<crate::core::fetch::Paging>> = Arc::new(Mutex::new(crate::core::fetch::Paging {
        cat: 0, offset: 0, epoch: 0, end: false, loading: false, items: Vec::new(),
    }));
    // cache：分类 -> 已加载结果（items / offset / end），切回该分类时直接恢复
    let cache: Arc<Mutex<HashMap<i32, (Vec<Row>, u32, bool)>>> = Arc::new(Mutex::new(HashMap::new()));

    // 从行元组重建 UI 模型：内存缓存命中的封面直接带上，其余由 fetch_covers 补
    fn rebuild_rows(items: &[Row]) -> Vec<crate::Playlist> {
        items.iter().map(|(id, name, plays, url, from, to)| crate::Playlist {
            id: *id, name: name.clone(), plays: plays.clone(),
            cover: crate::load::cover_now(url), from: *from, to: *to,
        }).collect()
    }

    /// 模型建好后补齐内存缓存里缺失的封面（切回已缓存分类时用；同 URL 由 cover_warm 合并）。
    /// 切回分类不再发网络请求，但封面必须有：命中 dcache 立即回填，未命中的磁盘/网络补齐。
    fn warm_missing_covers(
        u: &AppWindow,
        items: &[Row],
        paging: &Arc<Mutex<crate::core::fetch::Paging>>,
        my_gen: u64,
    ) {
        let rows: Vec<(usize, String, crate::persist::cache::CoverType)> = items.iter().enumerate()
            .filter(|(_, it)| !it.3.is_empty() && crate::load::cover_now(&it.3).size().width == 0)
            .map(|(i, it)| (i, it.3.to_string(), crate::persist::cache::CoverType::Playlist))
            .collect();
        if rows.is_empty() { return; }
        let model = u.get_playlists_all();
        let paging = paging.clone();
        crate::load::warm_rows(rows, Rc::new(move |i, img| {
            if paging.lock().unwrap().epoch != my_gen { return; }   // 已再次切走：丢弃
            if let Some(mut row) = model.row_data(i) {
                row.cover = img;
                model.set_row_data(i, row);
            }
        }));
    }

    // 加载一页（append=false 首屏覆盖，true 追加下一页）；封面渐变占位，UI 线程构造
    fn load_playlist_page(
        client: &Arc<crate::network::netease::client::NeteaseClient>,
        rt_h: &tokio::runtime::Handle,
        ui_weak: &slint::Weak<AppWindow>,
        live: &Arc<Mutex<crate::core::fetch::Paging>>,
        cat: i32,
        cat_name: String,
        is_daily: bool,
        offset: u32,
        append: bool,
    ) {
        let client = client.clone();
        let rt_h = rt_h.clone();
        let ui_task = ui_weak.clone();
        let pg = live.clone();
        let my_gen = { pg.lock().unwrap().epoch };
        rt_h.clone().spawn(async move {
            let rp = if is_daily {
                if append {
                    crate::network::service::top_playlists_by_cat(&client, "全部", 30, offset).await
                } else {
                    crate::network::service::personalized(&client).await
                }
            } else {
                crate::network::service::top_playlists_by_cat(&client, &cat_name, 30, offset).await
            };
            let path = if is_daily && !append { "/result" } else { "/playlists" };
            let fetched: Vec<Row> = rp
                .as_ref().ok()
                .and_then(|v| v.pointer(path))
                .and_then(|a| a.as_array())
                .map(|arr| arr.iter().enumerate().map(|(i, p)| {
                    // 歌单 id 已超过 32 位：登记真实 id，详情页点击时再还原
                    let id = crate::core::ids::remember(0, p.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                    let name: slint::SharedString = p.get("name").and_then(|v| v.as_str()).unwrap_or("").into();
                    let plays: slint::SharedString = fmt_plays_cat(p.get("playCount").and_then(|v| v.as_u64()).unwrap_or(0)).into();
                    let url: slint::SharedString = p.get("coverImgUrl").and_then(|v| v.as_str()).unwrap_or("").into();
                    (id, name, plays, url, crate::load::from(i), crate::load::to(i))
                }).collect()).unwrap_or_default();
            let ui_c = ui_task.clone();
            let pg_c = pg.clone();
            let page_items = fetched.clone();
            let base = offset as usize;
            let rows_items = page_items.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = ui_task.upgrade() else { return };
                {
                    let mut pg = pg.lock().unwrap();
                    if pg.cat != cat || pg.epoch != my_gen { return; }   // 已切走/过期：丢弃
                    if fetched.is_empty() { pg.end = true; u.set_playlists_no_more(true); }
                    if append { pg.items.extend(fetched.iter().cloned()); } else { pg.items = fetched.clone(); }
                    pg.offset = offset + 30;
                    pg.loading = false;
                    u.set_playlists_loading_more(false);
                    u.set_playlists_need_more(false);   // 复位，回到"滚动触底再加载"而非连拉到底
                }
                let rows = rebuild_rows(&rows_items);
                if append {
                    let model = u.get_playlists_all();
                    for it in rows { let _ = model.push_row(it); }
                } else {
                    u.set_playlists_all(Rc::new(VecModel::from(rows)).into());
                }
            });
            let page_urls: Vec<(String, crate::persist::cache::CoverType)> = page_items.iter()
                .map(|(_, _, _, url, _, _)| (url.to_string(), crate::persist::cache::CoverType::Playlist))
                .filter(|(u, _)| !u.is_empty())
                .collect();
            if !page_urls.is_empty() {
                rt_h.clone().spawn(async move {
                    let covers = crate::load::fetch_covers(&page_urls).await;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_c.upgrade() {
                            let g = pg_c.lock().unwrap();
                            if g.cat != cat || g.epoch != my_gen { return; }
                            drop(g);
                            let model = u.get_playlists_all();
                            for (i, (_, _, _, url, _, _)) in page_items.iter().enumerate() {
                                let Some(buf) = covers.get(url.as_str()).and_then(|x| x.as_ref()) else { continue };
                                let idx = base + i;
                                if let Some(mut row) = model.row_data(idx) {
                                    row.cover = slint::Image::from_rgba8(buf.clone());
                                    model.set_row_data(idx, row);
                                }
                            }
                        }
                    });
                });
            }
        });
    }

    // 点击分类：同分类跳过；已缓存则恢复；否则首屏加载
    ctx.app.clone_strong().on_playlist_cat_clicked({
        let ui_weak = ctx.ui.clone();
        let client_cb = ctx.client.clone();
        let rt_h = ctx.rt.clone();
        let live = live.clone();
        let cache = cache.clone();
        move |idx| {
            let client = client_cb.clone();
            let rt_h = rt_h.clone();
            let ui_weak = ui_weak.clone();
            let live = live.clone();
            let cache = cache.clone();
            // 取分类名与「是否每日推荐」需在 UI 线程读
            let is_daily = idx == 0;
            let cat_name = if is_daily {
                "每日推荐".to_string()
            } else {
                ui_weak.upgrade()
                    .and_then(|u| u.get_playlist_categories().row_data(idx as usize).map(|s| s.to_string()))
                    .unwrap_or_default()
            };
            if let Some(u) = ui_weak.upgrade() {
                u.set_playlists_no_more(false);
                u.set_playlists_loading_more(false);
                u.set_playlists_need_more(false);   // 关键：复位旧分类残留的补页信号
            }
            let mut pg = live.lock().unwrap();
            // 点击当前已激活分类：跳过重复刷新
            if idx == pg.cat && !pg.loading && !pg.items.is_empty() {
                return;
            }
            // 保存当前分类结果进缓存
            if !pg.items.is_empty() {
                cache.lock().unwrap().insert(pg.cat, (pg.items.clone(), pg.offset, pg.end));
            }
            // 切回已缓存分类：直接恢复，不重拉
            if let Some((items, offset, end)) = cache.lock().unwrap().remove(&idx) {
                pg.cat = idx; pg.epoch += 1; pg.offset = offset; pg.end = end;
                pg.loading = false; pg.items = items.clone();
                let my_gen = pg.epoch;
                drop(pg);
                let live_c = live.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_weak.upgrade() {
                        let rows = rebuild_rows(&items);
                        u.set_playlists_all(Rc::new(VecModel::from(rows)).into());
                        u.set_playlists_no_more(end);
                        warm_missing_covers(&u, &items, &live_c, my_gen);
                    }
                });
                return;
            }
            // 全新分类：重置并首屏加载
            pg.cat = idx; pg.epoch += 1; pg.offset = 0; pg.end = false;
            pg.items.clear(); pg.loading = true;
            drop(pg);
            load_playlist_page(&client, &rt_h, &ui_weak, &live, idx, cat_name, is_daily, 0, false);
        }
    });

    // 歌单触底分页轮询
    let pager_timer = Rc::new(slint::Timer::default());
    let client_p = ctx.client.clone();
    let rt_p = ctx.rt.clone();
    let ui_p = ctx.ui.clone();
    let pg_p = live.clone();
    pager_timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
        if let Some(u) = ui_p.upgrade() {
            let need = u.get_playlists_need_more();
            let (cat, offset, end, loading) = { let pg = pg_p.lock().unwrap(); (pg.cat, pg.offset, pg.end, pg.loading) };
            if need && !end && !loading {
                pg_p.lock().unwrap().loading = true;
                u.set_playlists_loading_more(true);
                let is_daily = cat == 0;
                let cat_name = if is_daily {
                    "每日推荐".to_string()
                } else {
                    u.get_playlist_categories().row_data(cat as usize).map(|s| s.to_string()).unwrap_or_default()
                };
                load_playlist_page(&client_p, &rt_p, &ui_p, &pg_p, cat, cat_name, is_daily, offset, true);
            }
        }
    });
    std::mem::forget(pager_timer);
}
