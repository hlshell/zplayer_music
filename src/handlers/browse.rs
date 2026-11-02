//! 浏览页数据回调：电台（个性化推荐电台列表 + 节目池，触底分页）与 MV（全部 MV 列表 + 触底分页）。
//! paging 状态为本模块内部持有，Timer 存活到 setup 作用域（mem::forget 防 drop 即停）。
//!
//! 接口对齐说明（实测 2026-10-23）：
//! - MV：old 用 /mv/all（公共 GET）。网抑云服务端当前忽略 area 过滤，故固定 area="" 取全部。
//! - 电台：/weapi/dj/catelist、/dj/recommend/type、/dj/radio/hot、/dj/program 已被服务端停用
//!   （均返回 404「接口未找到」）。唯一仍可用的浏览源是 /weapi/personalized/djprogram，
//!   返回推荐节目，每条含 program.radio（电台元信息）+ program.mainSong（音频）。
//!   据此按电台去重成电台列表，并保留节目池供详情页(kind=3)读取。

use super::Ctx;
use crate::AppWindow;
use crate::core::song_util::{fmt_dur, fmt_plays_cat};
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 电台行元组（含分类）：cat,name,id,desc,plays,url,from,to
type RadioRow = (slint::SharedString, i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color);
/// 电台状态：cat=当前分类索引（0=全部），all=全量去重电台列表；无分页（一次拉足，客户端过滤）
/// render_gen：每次重建列表递增，用于丢弃上一次渲染的迟到补图回调（避免行错位覆盖）
struct RadioPaging { cat: i32, epoch: u64, render_gen: u64, end: bool, all: Vec<RadioRow> }
/// MV 分页状态（完整 Send 元组：id,name,artist,duration,plays,url,from,to）
struct MvPaging { cat: i32, offset: u32, epoch: u64, end: bool, loading: bool, items: Vec<(i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color)> }

/// 电台节目池：dj_personalized_recommend 返回的完整节目（含 mainSong）按电台 id 分组，
/// 供详情页(kind=3)读取真实节目（/weapi/dj/program 已被封锁）。
pub static RADIO_PROGRAMS: Mutex<Vec<(i64, Vec<serde_json::Value>)>> = Mutex::new(Vec::new());

pub fn setup(ctx: &Ctx) {
    // ---- 电台：个性化推荐节目 → 按电台去重成电台列表；按 radio.category 建分类 Tab（客户端过滤，零额外请求）----
    let radio_paging: Arc<Mutex<RadioPaging>> = Arc::new(Mutex::new(RadioPaging { cat: 0, epoch: 0, render_gen: 0, end: true, all: Vec::new() }));

    // 按当前 cat 过滤 all 并重建电台模型（UI 线程）+ 补封面（后台 fetch_covers，磁盘有缓存）
    fn render_radio_filter(
        rt_h: &tokio::runtime::Handle,
        ui_weak: &slint::Weak<AppWindow>,
        pg: &Arc<Mutex<RadioPaging>>,
    ) {
        // 每次重渲染递增 render_gen：上一次渲染的迟到回调据此丢弃
        let (all, cat, my_gen) = {
            let mut p = pg.lock().unwrap();
            p.render_gen += 1;
            (p.all.clone(), p.cat, p.render_gen)
        };
        let rt_h = rt_h.clone();
        let ui_weak = ui_weak.clone();
        let pg = pg.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            if pg.lock().unwrap().render_gen != my_gen { return; }
            let filter = u.get_radio_categories().row_data(cat as usize).map(|s| s.to_string()).unwrap_or_default();
            // 「全部」(索引 0) 为通配：不过滤；其余按分类名精确匹配
            let is_all = cat == 0 || filter.is_empty() || filter == "全部";
            // 保留 url 供补图；已过滤元组
            let filtered: Vec<(RadioRow, slint::SharedString)> = all.iter()
                .filter(|r| is_all || r.0.as_str() == filter.as_str())
                .map(|r| (r.clone(), r.5.clone()))
                .collect();
            let rows: Vec<crate::Radio> = filtered.iter().map(|(r, _)| crate::Radio {
                id: r.1, name: r.2.clone(), desc: r.3.clone(), plays: r.4.clone(),
                cover: crate::load::cover_now(&r.5), from: r.6, to: r.7,
            }).collect();
            u.set_radios(Rc::new(VecModel::from(rows)).into());
            let page_urls: Vec<(String, crate::persist::cache::CoverType)> = filtered.iter()
                .map(|(_, url)| (url.to_string(), crate::persist::cache::CoverType::Radio))
                .filter(|(u, _)| !u.is_empty())
                .collect();
            if !page_urls.is_empty() {
                let rt_h2 = rt_h.clone();
                let ui_c = ui_weak.clone();
                let pg_c = pg.clone();
                rt_h2.spawn(async move {
                    let covers = crate::load::fetch_covers(&page_urls).await;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_c.upgrade() {
                            if pg_c.lock().unwrap().render_gen != my_gen { return; }   // 已重渲染：丢弃
                            let model = u.get_radios();
                            for (i, (_row, url)) in filtered.iter().enumerate() {
                                let Some(buf) = covers.get(url.as_str()).and_then(|x| x.as_ref()) else { continue };
                                if let Some(mut r) = model.row_data(i) {
                                    r.cover = slint::Image::from_rgba8(buf.clone());
                                    model.set_row_data(i, r);
                                }
                            }
                        }
                    });
                });
            }
        });
    }

    // 加载电台（一次拉足推荐节目，无 offset 分页；按 radio.id 去重，派生分类 Tab）
    fn load_radio_page(
        client: &Arc<crate::network::netease::client::NeteaseClient>,
        rt_h: &tokio::runtime::Handle,
        ui_weak: &slint::Weak<AppWindow>,
        paging: &Arc<Mutex<RadioPaging>>,
    ) {
        let client = client.clone();
        let rt_h = rt_h.clone();
        let ui_task = ui_weak.clone();
        let pg = paging.clone();
        let my_gen = { pg.lock().unwrap().epoch };
        rt_h.clone().spawn(async move {
            let rp = crate::network::service::dj_personalized_recommend(&client, 80).await;
            let result = rp
                .ok()
                .and_then(|v| v.get("result").and_then(|a| a.as_array()).cloned())
                .unwrap_or_default();
            // 节目池：按 program.radio.id 分组（完整 program，含 mainSong 音频），供详情页读取
            let mut prog_map: std::collections::HashMap<i64, Vec<serde_json::Value>> = Default::default();
            for it in &result {
                if let Some(prog) = it.get("program") {
                    if let Some(rid) = prog.get("radio").and_then(|r| r.get("id")).and_then(|i| i.as_i64()) {
                        prog_map.entry(rid).or_default().push(prog.clone());
                    }
                }
            }
            *RADIO_PROGRAMS.lock().unwrap() = prog_map.into_iter().collect();
            // 电台去重列表（含分类）：cat,id,name,desc,plays(订阅数),url,from,to
            let mut seen: std::collections::HashSet<i64> = Default::default();
            let mut all: Vec<RadioRow> = Vec::new();
            let mut cat_count: std::collections::HashMap<String, u32> = Default::default();
            for it in &result {
                let Some(prog) = it.get("program") else { continue };
                let Some(radio) = prog.get("radio") else { continue };
                let id = radio.get("id").and_then(|i| i.as_i64()).unwrap_or(0);
                if id == 0 || !seen.insert(id) { continue; }
                let cat: slint::SharedString = radio.get("category").and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty()).unwrap_or("其他").into();
                let name: slint::SharedString = radio.get("name").and_then(|v| v.as_str()).unwrap_or("").into();
                let desc: slint::SharedString = radio.get("desc").and_then(|v| v.as_str()).unwrap_or("").into();
                let plays: slint::SharedString = fmt_plays_cat(radio.get("subCount").and_then(|v| v.as_u64()).unwrap_or(0)).into();
                let url: slint::SharedString = radio.get("picUrl").and_then(|v| v.as_str()).unwrap_or("").into();
                let i = all.len();
                all.push((cat.clone(), crate::core::ids::remember(3, id), name, desc, plays, url, crate::load::from(i), crate::load::to(i)));
                *cat_count.entry(cat.to_string()).or_insert(0) += 1;
            }
            // 派生分类 Tab：全部 + 各分类（按条数降序）
            let mut cats: Vec<String> = cat_count.keys().cloned().collect();
            cats.sort_by_key(|c| std::cmp::Reverse(cat_count[c]));
            let mut cats_disp: Vec<slint::SharedString> = vec![slint::SharedString::from("全部")];
            cats_disp.extend(cats.into_iter().map(|c| slint::SharedString::from(c)));
            let ui_c = ui_task.clone();
            let pg_c = pg.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = ui_task.upgrade() else { return };
                {
                    let mut pg = pg.lock().unwrap();
                    if pg.epoch != my_gen { return; }
                    pg.all = all.clone();
                    pg.end = true;
                    u.set_radios_no_more(true);
                    u.set_radios_loading_more(false);
                    u.set_radios_need_more(false);
                }
                u.set_radio_categories(Rc::new(VecModel::from(cats_disp)).into());
                // 按当前分类过滤渲染
                render_radio_filter(&rt_h, &ui_c, &pg_c);
            });
        });
    }

    // 电台首屏加载（默认"全部"）
    {
        let client_cb = ctx.client.clone();
        let rt_h = ctx.rt.clone();
        let ui_w = ctx.ui.clone();
        let pg = radio_paging.clone();
        load_radio_page(&client_cb, &rt_h, &ui_w, &pg);
    }

    // 点击电台分类：客户端过滤（零额外请求），不重拉数据
    ctx.app.clone_strong().on_radio_cat_clicked({
        let ui_weak = ctx.ui.clone();
        let rt_h = ctx.rt.clone();
        let pg = radio_paging.clone();
        move |idx| {
            let rt_h = rt_h.clone();
            let ui_weak = ui_weak.clone();
            let pg = pg.clone();
            {
                let mut p = pg.lock().unwrap();
                p.cat = idx;
            }
            render_radio_filter(&rt_h, &ui_weak, &pg);
        }
    });

    // ---- MV：全部MV(/api/mv/all) + 精选MV(/weapi/mv/exclusive/rcmd) 两分类 Tab ----
    // 网抑云忽略 /mv/all 的 area 过滤（各分类返回相同列表），故 MV 分类用两个真实不同源。
    // 分类按需缓存：切回已加载分类直接恢复，不重拉。
    type MvRow = (i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color);
    let mv_paging: Arc<Mutex<MvPaging>> = Arc::new(Mutex::new(MvPaging { cat: 0, offset: 0, epoch: 0, end: false, loading: false, items: Vec::new() }));
    let mv_cache: Arc<Mutex<std::collections::HashMap<i32, (Vec<MvRow>, u32, bool)>>> = Arc::new(Mutex::new(std::collections::HashMap::new()));

    // 设置 MV 分类 Tab（全部MV / 精选MV）
    {
        let ui_w = ctx.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_w.upgrade() {
                u.set_mv_categories(Rc::new(VecModel::from(vec![
                    slint::SharedString::from("全部MV"),
                    slint::SharedString::from("精选MV"),
                ])).into());
            }
        });
    }

    // 把 MV 列表同步进播放队列（prev/next 数据源）。保留当前播放 id 的定位。
    fn sync_mv_queue(items: &[MvRow]) {
        let q = crate::handlers::mv::queue::queue();
        let cur_id = q.lock().unwrap().get_current().map(|c| c.id);
        let qitems: Vec<crate::handlers::mv::queue::MvQueueItem> = items.iter().map(|(id, name, artist, dur, _, url, from, to)| {
            crate::handlers::mv::queue::MvQueueItem {
                id: crate::core::ids::resolve(4, *id),
                name: name.to_string(),
                artist: artist.to_string(),
                cover_url: url.to_string(),
                duration: dur.to_string(),
                from: color_to_u32(*from),
                to: color_to_u32(*to),
            }
        }).collect();
        // 如果当前播放的 MV 不在新队列里（来自搜索/收藏夹等外部来源），
        // 不替换队列，避免 current 被意外重置导致"跳歌"
        let new_idx = cur_id.and_then(|id| qitems.iter().position(|x| x.id == id));
        if cur_id.is_some() && new_idx.is_none() {
            return;
        }
        let mut g = q.lock().unwrap();
        g.set_items(qitems, new_idx.unwrap_or(0));
    }

    fn color_to_u32(c: slint::Color) -> u32 {
        let rgba = c.to_argb_u8();
        ((rgba.alpha as u32) << 24) | ((rgba.red as u32) << 16) | ((rgba.green as u32) << 8) | (rgba.blue as u32)
    }

    fn load_mv_page(
        client: &Arc<crate::network::netease::client::NeteaseClient>,
        rt_h: &tokio::runtime::Handle,
        ui_weak: &slint::Weak<AppWindow>,
        paging: &Arc<Mutex<MvPaging>>,
        cat: i32,
        offset: u32,
        append: bool,
    ) {
        let client = client.clone();
        let rt_h = rt_h.clone();
        let ui_task = ui_weak.clone();
        let pg = paging.clone();
        let my_gen = pg.lock().unwrap().epoch;
        rt_h.clone().spawn(async move {
            // 分类 0=全部MV(mv_all)，1=精选MV(mv_exclusive)，字段一致（id/name/artistName/cover/playCount/duration）
            let rp = if cat == 0 {
                crate::network::service::mv_all(&client, "", 30, offset).await
            } else {
                crate::network::service::mv_exclusive(&client, 30, offset).await
            };
            let ui_items: Vec<MvRow> = rp
                .ok()
                .and_then(|v| v.get("data").and_then(|a| a.as_array()).cloned())
                .map(|arr| {
                    arr.iter().enumerate().map(|(i, m)| {
                        let id = crate::core::ids::remember(4, m.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                        let name: slint::SharedString = m.get("name").and_then(|v| v.as_str()).unwrap_or("").into();
                        let artist: slint::SharedString = m.get("artistName").and_then(|v| v.as_str()).unwrap_or("").into();
                        let dur_ms = m.get("duration").and_then(|v| v.as_u64()).unwrap_or(0);
                        let duration: slint::SharedString = fmt_dur(dur_ms / 1000).into();
                        let plays: slint::SharedString = fmt_plays_cat(m.get("playCount").and_then(|v| v.as_u64()).unwrap_or(0)).into();
                        let url: slint::SharedString = m.get("cover").and_then(|v| v.as_str())
                            .or_else(|| m.get("imgurl").and_then(|v| v.as_str())).unwrap_or("").into();
                        (id, name, artist, duration, plays, url, crate::load::from(i), crate::load::to(i))
                    }).collect()
                }).unwrap_or_default();
            let ui_c = ui_task.clone();
            let pg_c = pg.clone();
            let page_items = ui_items.clone();
            let base = offset as usize;
            let rows_items = page_items.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = ui_task.upgrade() else { return };
                {
                    let mut pg = pg.lock().unwrap();
                    if pg.cat != cat || pg.epoch != my_gen { return; }   // 已切走/过期：丢弃
                    if ui_items.is_empty() { pg.end = true; u.set_mvs_no_more(true); }
                    if append { pg.items.extend(ui_items.iter().cloned()); } else { pg.items = ui_items.clone(); }
                    pg.offset = offset + 30;
                    pg.loading = false;
                    u.set_mvs_loading_more(false);
                    u.set_mvs_need_more(false);   // 复位，回到"滚动触底再加载"
                    sync_mv_queue(&pg.items);
                }
                // 同步抽屉 UI（分页加载后刷新播放列表卡片；封面后台异步填充）
                crate::handlers::mv::queue::sync_ui(&u);
                // 封面占位，解码/下载全部交给后台 fetch_covers（UI 线程同步解码会卡死）
                let rows: Vec<crate::MvItem> = rows_items.iter().map(|(id, name, artist, duration, plays, url, from, to)| crate::MvItem {
                    id: *id, name: name.clone(), artist: artist.clone(), duration: duration.clone(),
                    plays: plays.clone(), url: url.clone().into(),
                    cover: slint::Image::default(),
                    from: *from, to: *to,
                }).collect();
                if append {
                    let model = u.get_mvs();
                    for it in rows { let _ = model.push_row(it); }
                } else {
                    u.set_mvs(Rc::new(VecModel::from(rows)).into());
                }
            });
            // 网络补图：仅刷新本页行封面（set_row_data），不重建全量模型
            let page_urls: Vec<(String, crate::persist::cache::CoverType)> = page_items.iter()
                .map(|(_, _, _, _, _, url, _, _)| (url.to_string(), crate::persist::cache::CoverType::Mv))
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
                            let model = u.get_mvs();
                            for (i, (_, _, _, _, _, url, _, _)) in page_items.iter().enumerate() {
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

    // 从 MV 行重建模型：内存缓存命中的封面直接带上，其余由 warm_missing_mv_covers 补齐
    fn rebuild_mv_rows(items: &[MvRow]) -> Vec<crate::MvItem> {
        items.iter().map(|(id, name, artist, duration, plays, url, from, to)| crate::MvItem {
            id: *id, name: name.clone(), artist: artist.clone(), duration: duration.clone(),
            plays: plays.clone(), url: url.clone().into(), cover: crate::load::cover_now(url),
            from: *from, to: *to,
        }).collect()
    }

    // 补齐内存缓存缺失的封面（切回已缓存分类时用；同 URL 由 cover_warm 合并）
    fn warm_missing_mv_covers(
        u: &AppWindow,
        items: &[MvRow],
        paging: &Arc<Mutex<MvPaging>>,
        my_gen: u64,
    ) {
        let rows: Vec<(usize, String, crate::persist::cache::CoverType)> = items.iter().enumerate()
            .filter(|(_, it)| !it.5.is_empty() && crate::load::cover_now(&it.5).size().width == 0)
            .map(|(i, it)| (i, it.5.to_string(), crate::persist::cache::CoverType::Mv))
            .collect();
        if rows.is_empty() { return; }
        let model = u.get_mvs();
        let paging = paging.clone();
        crate::load::warm_rows(rows, Rc::new(move |i, img| {
            if paging.lock().unwrap().epoch != my_gen { return; }   // 已再次切走：丢弃
            if let Some(mut row) = model.row_data(i) {
                row.cover = img;
                model.set_row_data(i, row);
            }
        }));
    }

    // MV 首屏加载（全部MV）
    {
        let client_cb = ctx.client.clone();
        let rt_h = ctx.rt.clone();
        let ui_w = ctx.ui.clone();
        let pg = mv_paging.clone();
        load_mv_page(&client_cb, &rt_h, &ui_w, &pg, 0, 0, false);
    }

    // MV 分类切换：同分类跳过；已缓存恢复；否则首屏加载
    ctx.app.clone_strong().on_mv_cat_clicked({
        let ui_weak = ctx.ui.clone();
        let client_cb = ctx.client.clone();
        let rt_h = ctx.rt.clone();
        let live = mv_paging.clone();
        let cache = mv_cache.clone();
        move |idx| {
            let client = client_cb.clone();
            let rt_h = rt_h.clone();
            let ui_weak = ui_weak.clone();
            let live = live.clone();
            let cache = cache.clone();
            if let Some(u) = ui_weak.upgrade() {
                u.set_mvs_no_more(false);
                u.set_mvs_loading_more(false);
                u.set_mvs_need_more(false);   // 复位旧分类残留补页信号
            }
            let mut pg = live.lock().unwrap();
            // 点击当前已激活分类：跳过
            if idx == pg.cat && !pg.loading && !pg.items.is_empty() { return; }
            // 保存当前分类结果
            if !pg.items.is_empty() {
                cache.lock().unwrap().insert(pg.cat, (pg.items.clone(), pg.offset, pg.end));
            }
            // 切回已缓存分类：直接恢复（封面按内存缓存回填，缺失的由 warm_missing_mv_covers 补齐）
            if let Some((items, offset, end)) = cache.lock().unwrap().remove(&idx) {
                pg.cat = idx; pg.epoch += 1; pg.offset = offset; pg.end = end;
                pg.loading = false; pg.items = items.clone();
                let my_gen = pg.epoch;
                drop(pg);
                sync_mv_queue(&items);
                let live_c = live.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_weak.upgrade() {
                        if live_c.lock().unwrap().epoch != my_gen { return; }
                        let rows = rebuild_mv_rows(&items);
                        u.set_mvs(Rc::new(VecModel::from(rows)).into());
                        u.set_mvs_no_more(end);
                        warm_missing_mv_covers(&u, &items, &live_c, my_gen);
                    }
                });
                return;
            }
            // 全新分类：重置并首屏加载
            pg.cat = idx; pg.epoch += 1; pg.offset = 0; pg.end = false;
            pg.items.clear(); pg.loading = true;
            drop(pg);
            load_mv_page(&client, &rt_h, &ui_weak, &live, idx, 0, false);
        }
    });

    // MV 触底分页轮询
    {
        let mv_timer = Rc::new(slint::Timer::default());
        let client_p = ctx.client.clone();
        let rt_p = ctx.rt.clone();
        let ui_p = ctx.ui.clone();
        let pg_p = mv_paging.clone();
        mv_timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
            if let Some(u) = ui_p.upgrade() {
                let need = u.get_mvs_need_more();
                let (cat, offset, end, loading) = { let pg = pg_p.lock().unwrap(); (pg.cat, pg.offset, pg.end, pg.loading) };
                if need && !end && !loading {
                    pg_p.lock().unwrap().loading = true;
                    u.set_mvs_loading_more(true);
                    load_mv_page(&client_p, &rt_p, &ui_p, &pg_p, cat, offset, true);
                }
            }
        });
        std::mem::forget(mv_timer);
    }
}
