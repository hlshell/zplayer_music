//! 专辑页回调：分类切换（全部/华语/欧美/韩国/日本）+ 触底分页。

use super::Ctx;
use crate::AppWindow;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;
use std::time::Duration;

/// 专辑行元组：id,name,artist,year,url,from,to
type AlbumRow = (i32, slint::SharedString, slint::SharedString, slint::SharedString, slint::SharedString, slint::Color, slint::Color);

/// 专辑分页状态（滚动触底追加下一页）。items 只存 Send 字段。
struct AlbumPaging {
    area_idx: i32,
    area: String,
    offset: u32,
    /// 分类切换代数：旧分类的迟到响应据此丢弃
    epoch: u64,
    end: bool,
    loading: bool,
    items: Vec<AlbumRow>,
}

/// 专辑分类：标签 + 网易云 /album/new 的 area 值（ALL/ZH/EA/KR/JP）
const ALBUM_AREAS: [(&str, &str); 5] = [
    ("全部", "ALL"), ("华语", "ZH"), ("欧美", "EA"), ("韩国", "KR"), ("日本", "JP"),
];

/// publishTime（毫秒时间戳）→ 年份字符串
fn ms_to_year_shared(ms: i64) -> slint::SharedString {
    let days = ms / 1000 / 86400;
    (1970 + (days as f64 / 365.25) as i64).to_string().into()
}

pub(crate) fn setup_albums(ctx: &Ctx) {
    let album_paging = std::sync::Arc::new(std::sync::Mutex::new(AlbumPaging {
        area_idx: 0, area: "ALL".to_string(), offset: 0, epoch: 0, end: false, loading: false, items: Vec::new(),
    }));
    // 分类 -> 已加载结果（items / offset / end），切回该分类时直接恢复，封面按 dcache 回填
    let album_cache: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i32, (Vec<AlbumRow>, u32, bool)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // 从行元组重建 UI 模型：内存缓存命中的封面直接带上，避免切回分类只剩占位色
    fn rebuild_album_rows(items: &[AlbumRow]) -> Vec<crate::Album> {
        items.iter().map(|(id, name, artist, year, url, from, to)| crate::Album {
            id: *id, name: name.clone(), artist: artist.clone(), year: year.clone(),
            cover: crate::load::cover_now(url), from: *from, to: *to,
        }).collect()
    }

    // 补齐内存缓存缺失的封面（切回已缓存分类时用；命中 dcache 立即回填，未命中后台补齐）
    fn warm_missing_album_covers(
        u: &AppWindow,
        items: &[AlbumRow],
        paging: &std::sync::Arc<std::sync::Mutex<AlbumPaging>>,
        my_gen: u64,
    ) {
        let rows: Vec<(usize, String, crate::persist::cache::CoverType)> = items.iter().enumerate()
            .filter(|(_, it)| !it.4.is_empty() && crate::load::cover_now(&it.4).size().width == 0)
            .map(|(i, it)| (i, it.4.to_string(), crate::persist::cache::CoverType::Album))
            .collect();
        if rows.is_empty() { return; }
        let model = u.get_album_list();
        let paging = paging.clone();
        crate::load::warm_rows(rows, Rc::new(move |i, img| {
            if paging.lock().unwrap().epoch != my_gen { return; }
            if let Some(mut row) = model.row_data(i) {
                row.cover = img;
                model.set_row_data(i, row);
            }
        }));
    }

    // 加载一页专辑（append=false 首屏覆盖，true 追加下一页）；封面先查内存缓存，UI 线程构造
    fn load_album_page(
        client: &std::sync::Arc<crate::network::netease::client::NeteaseClient>,
        rt_h: &tokio::runtime::Handle,
        ui_weak: &slint::Weak<AppWindow>,
        pg: &std::sync::Arc<std::sync::Mutex<AlbumPaging>>,
        area: String,
        offset: u32,
        append: bool,
    ) {
        let client = client.clone();
        let rt_h = rt_h.clone();
        let ui_task = ui_weak.clone();
        let pg2 = pg.clone();
        let my_gen = pg2.lock().unwrap().epoch;
        rt_h.clone().spawn(async move {
            let rp = crate::network::service::new_albums(&client, &area, 30, offset).await;
            let fetched: Vec<AlbumRow> = rp
                .as_ref().ok()
                .and_then(|v| v.pointer("/albums").and_then(|a| a.as_array()))
                .map(|arr| arr.iter().enumerate().map(|(i, al)| {
                    let id = crate::core::ids::remember(1, al.get("id").and_then(|v| v.as_i64()).unwrap_or(0));
                    let name: slint::SharedString = al.get("name").and_then(|v| v.as_str()).unwrap_or("").into();
                    let artist: slint::SharedString = al.get("artist")
                        .and_then(|x| x.get("name")).and_then(|v| v.as_str()).unwrap_or("").into();
                    let year: slint::SharedString = al.get("publishTime").and_then(|v| v.as_i64())
                        .map(ms_to_year_shared).unwrap_or_default();
                    let url: slint::SharedString = al.get("picUrl").and_then(|v| v.as_str()).unwrap_or("").into();
                    (id, name, artist, year, url, crate::load::from(i), crate::load::to(i))
                }).collect()).unwrap_or_default();
            let ui_c = ui_task.clone();
            let pg_c = pg2.clone();
            let page_items = fetched.clone();
            let base = offset as usize;
            let rows_items = page_items.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(u) = ui_task.upgrade() else { return };
                {
                    let mut p = pg2.lock().unwrap();
                    if p.epoch != my_gen { return; }
                    if fetched.is_empty() { p.end = true; u.set_albums_no_more(true); }
                    if append { p.items.extend(fetched.iter().cloned()); } else { p.items = fetched.clone(); }
                    p.offset = offset + 30;
                    p.loading = false;
                    u.set_albums_loading_more(false);
                    u.set_albums_need_more(false);
                }
                let rows = rebuild_album_rows(&rows_items);
                if append {
                    let model = u.get_album_list();
                    for it in rows { let _ = model.push_row(it); }
                } else {
                    u.set_album_list(Rc::new(VecModel::from(rows)).into());
                }
            });
            let page_urls: Vec<(String, crate::persist::cache::CoverType)> = page_items.iter()
                .map(|(_, _, _, _, url, _, _)| (url.to_string(), crate::persist::cache::CoverType::Album))
                .filter(|(u, _)| !u.is_empty())
                .collect();
            if !page_urls.is_empty() {
                rt_h.clone().spawn(async move {
                    let covers = crate::load::fetch_covers(&page_urls).await;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_c.upgrade() {
                            if pg_c.lock().unwrap().epoch != my_gen { return; }
                            let model = u.get_album_list();
                            for (i, (_, _, _, _, url, _, _)) in page_items.iter().enumerate() {
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

    // 初始填充分类 Tab + 首屏（全部）
    {
        let names: Vec<slint::SharedString> = ALBUM_AREAS.iter().map(|(label, _)| (*label).into()).collect();
        let weak0 = ctx.ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = weak0.upgrade() {
                u.set_album_categories(Rc::new(VecModel::from(names)).into());
            }
        });
        load_album_page(&ctx.client, &ctx.rt, &ctx.ui, &album_paging, "ALL".to_string(), 0, false);
    }

    ctx.app.clone_strong().on_album_cat_clicked({
        let ui_weak = ctx.ui.clone();
        let client = ctx.client.clone();
        let rt_h = ctx.rt.clone();
        let pg = album_paging.clone();
        let cache = album_cache.clone();
        move |idx| {
            let area = ALBUM_AREAS.get(idx as usize).map(|(_, a)| a.to_string()).unwrap_or("ALL".to_string());
            let ui_weak = ui_weak.clone();
            if let Some(u) = ui_weak.upgrade() {
                u.set_albums_no_more(false);
                u.set_albums_loading_more(false);
                u.set_albums_need_more(false);   // 复位旧分类残留的补页信号
            }
            let mut p = pg.lock().unwrap();
            // 点击当前已激活分类：跳过重复刷新
            if idx == p.area_idx && !p.loading && !p.items.is_empty() { return; }
            // 保存当前分类结果
            if !p.items.is_empty() {
                cache.lock().unwrap().insert(p.area_idx, (p.items.clone(), p.offset, p.end));
            }
            // 切回已缓存分类：直接恢复，不重拉；封面按内存缓存回填
            if let Some((items, offset, end)) = cache.lock().unwrap().remove(&idx) {
                p.area_idx = idx; p.area = area.clone(); p.epoch += 1;
                p.offset = offset; p.end = end; p.loading = false; p.items = items.clone();
                let my_gen = p.epoch;
                drop(p);
                let pg_c = pg.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_weak.upgrade() {
                        let rows = rebuild_album_rows(&items);
                        u.set_album_list(Rc::new(VecModel::from(rows)).into());
                        u.set_albums_no_more(end);
                        warm_missing_album_covers(&u, &items, &pg_c, my_gen);
                    }
                });
                return;
            }
            // 全新分类：重置并首屏加载
            p.area_idx = idx; p.area = area.clone(); p.epoch += 1; p.offset = 0; p.end = false; p.items.clear(); p.loading = true;
            drop(p);
            load_album_page(&client, &rt_h, &ui_weak, &pg, area, 0, false);
        }
    });

    // 专辑触底分页轮询
    let album_timer = Rc::new(slint::Timer::default());
    let client_a = ctx.client.clone();
    let rt_a = ctx.rt.clone();
    let ui_a = ctx.ui.clone();
    let pg_a = album_paging.clone();
    album_timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || {
        if let Some(u) = ui_a.upgrade() {
            let need = u.get_albums_need_more();
            let (area, offset, end, loading) = {
                let p = pg_a.lock().unwrap();
                (p.area.clone(), p.offset, p.end, p.loading)
            };
            if need && !end && !loading {
                pg_a.lock().unwrap().loading = true;
                u.set_albums_loading_more(true);
                load_album_page(&client_a, &rt_a, &ui_a, &pg_a, area, offset, true);
            }
        }
    });
    std::mem::forget(album_timer);
}
