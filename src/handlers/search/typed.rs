//! 分类搜索（专辑/歌单/MV/电台）：由 songs.rs 的 on_search_clicked / on_search_load_more 分派。
//! 结果填充 AppWindow 的 search-albums / search-playlists / search-mvs / search-radios。
//! 注意：slint::Image 非 Send，模型构造（含 cover_now）必须在 invoke_from_event_loop 内完成；
//! 封面缺失的由后台 fetch_covers 按全局索引回填。

use crate::AppWindow;
use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::Arc;

/// 卡片行纯数据（Send）：idx 为模型全局行号（追加模式下 = offset + 页内序号）。
#[derive(Clone)]
struct Row {
    id: i32,
    name: String,
    sub: String,
    badge: String,
    url: String,
    idx: usize,
    cover_type: crate::persist::cache::CoverType,
}

/// 执行分类搜索。ty: 1=专辑 2=歌单 3=MV 4=电台；append=true 追加到现有模型。
pub fn run(
    client: Arc<crate::network::netease::client::NeteaseClient>,
    rt: tokio::runtime::Handle,
    ui_weak: slint::Weak<AppWindow>,
    toast: Arc<crate::handlers::ToastCtx>,
    kw: String,
    ty: i32,
    offset: u32,
    append: bool,
) {
    let (search_type, label) = match ty {
        1 => (10, "张专辑"),
        2 => (1000, "个歌单"),
        3 => (1004, "个 MV"),
        4 => (1009, "个电台"),
        _ => return,
    };
    rt.spawn(async move {
        let raw = crate::network::service::search(&client, &kw, search_type, 30, offset as i64).await;
        let Ok(v) = raw else {
            super::songs::end_page(0, false);
            let ui_err = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                // 失败也要撤下骨架屏，否则会一直停在骨架状态
                if let Some(u) = ui_err.upgrade() { u.set_search_loading(false); }
                if !append { toast.show("搜索失败，请稍后重试", "error"); }
            });
            return;
        };
        // 解析为纯数据行（不含 Image，Send 安全）
        let rows: Vec<Row> = parse_rows(&v, ty, offset as usize);
        let n = rows.len();
        super::songs::end_page(n, n == 30);

        // UI 线程构造模型并填充（Row 不含 Image，可整体 move 进闭包）
        let ui2 = ui_weak.clone();
        let fill_rows = rows.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui2.upgrade() else { return };
            if append { append_model(&u, ty, &rows); } else { set_model(&u, ty, &rows); }
            // 数据到达，撤下骨架屏
            u.set_search_loading(false);
            if !append {
                toast.show(
                    &if n > 0 { format!("搜索到 {} {}", n, label) } else { format!("未找到相关{}", &label[1..]) },
                    if n > 0 { "info" } else { "error" },
                );
            }
        });

        // 后台补齐封面
        spawn_fill(ui_weak, ty, fill_rows);
    });
}

/// 按类型解析搜索结果为纯数据行。base 为本页起始的全局行号。
fn parse_rows(v: &serde_json::Value, ty: i32, base: usize) -> Vec<Row> {
    let arr = match ty {
        1 => v.pointer("/result/albums"),
        2 => v.pointer("/result/playlists"),
        3 => v.pointer("/result/mvs"),
        _ => v.pointer("/result/djRadios"),
    }.and_then(|a| a.as_array()).cloned().unwrap_or_default();

    arr.iter().enumerate().map(|(i, m)| {
        let s = |k: &str| m.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        match ty {
            1 => Row {   // 专辑
                id: crate::core::ids::remember(1, m.get("id").and_then(|x| x.as_i64()).unwrap_or(0)),
                name: s("name"),
                sub: m.pointer("/artist/name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                badge: m.get("publishTime").and_then(|x| x.as_i64())
                    .map(|ms| ((1970.0 + (ms / 1000 / 86400) as f64 / 365.25) as i64).to_string())
                    .unwrap_or_default(),
                url: s("picUrl"), idx: base + i, cover_type: crate::persist::cache::CoverType::Album,
            },
            2 => Row {   // 歌单
                id: crate::core::ids::remember(0, m.get("id").and_then(|x| x.as_i64()).unwrap_or(0)),
                name: s("name"),
                sub: String::new(),
                badge: crate::core::song_util::fmt_plays_cat(m.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0)),
                url: s("coverImgUrl"), idx: base + i, cover_type: crate::persist::cache::CoverType::Playlist,
            },
            3 => Row {   // MV
                id: crate::core::ids::remember(4, m.get("id").and_then(|x| x.as_i64()).unwrap_or(0)),
                name: s("name"),
                sub: m.get("artists").and_then(|a| a.as_array())
                    .map(|arr| arr.iter().filter_map(|a| a.get("name").and_then(|n| n.as_str())).collect::<Vec<_>>().join(" / "))
                    .unwrap_or_default(),
                badge: crate::core::song_util::fmt_dur(m.get("duration").and_then(|x| x.as_u64()).unwrap_or(0) / 1000).to_string(),
                url: s("cover"), idx: base + i, cover_type: crate::persist::cache::CoverType::Mv,
            },
            _ => Row {   // 电台
                id: crate::core::ids::remember(3, m.get("id").and_then(|x| x.as_i64()).unwrap_or(0)),
                name: s("name"),
                sub: m.pointer("/dj/nickname").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                badge: crate::core::song_util::fmt_plays_cat(m.get("subCount").and_then(|x| x.as_u64()).unwrap_or(0)),
                url: s("picUrl"), idx: base + i, cover_type: crate::persist::cache::CoverType::Radio,
            },
        }
    }).collect()
}

/// UI 线程内按类型构造模型并填充（cover_now 命中内存缓存直接带图）。
fn set_model(u: &AppWindow, ty: i32, rows: &[Row]) {
    match ty {
        1 => u.set_search_albums(Rc::new(VecModel::from(rows.iter().map(|r| crate::Album {
            id: r.id, name: r.name.as_str().into(), artist: r.sub.as_str().into(),
            year: r.badge.as_str().into(), cover: crate::load::cover_now(&r.url),
            from: crate::load::from(r.idx), to: crate::load::to(r.idx),
        }).collect::<Vec<_>>())).into()),
        2 => u.set_search_playlists(Rc::new(VecModel::from(rows.iter().map(|r| crate::Playlist {
            id: r.id, name: r.name.as_str().into(), plays: r.badge.as_str().into(),
            cover: crate::load::cover_now(&r.url),
            from: crate::load::from(r.idx), to: crate::load::to(r.idx),
        }).collect::<Vec<_>>())).into()),
        3 => u.set_search_mvs(Rc::new(VecModel::from(rows.iter().map(|r| crate::MvItem {
            id: r.id, name: r.name.as_str().into(), artist: r.sub.as_str().into(),
            duration: r.badge.as_str().into(), plays: "".into(),
            url: r.url.as_str().into(),
            cover: crate::load::cover_now(&r.url),
            from: crate::load::from(r.idx), to: crate::load::to(r.idx),
        }).collect::<Vec<_>>())).into()),
        _ => u.set_search_radios(Rc::new(VecModel::from(rows.iter().map(|r| crate::Radio {
            id: r.id, name: r.name.as_str().into(), desc: r.sub.as_str().into(),
            plays: r.badge.as_str().into(), cover: crate::load::cover_now(&r.url),
            from: crate::load::from(r.idx), to: crate::load::to(r.idx),
        }).collect::<Vec<_>>())).into()),
    }
}

/// 追加一页：downcast 到 VecModel 逐行 push（自动通知视图）。
fn append_model(u: &AppWindow, ty: i32, rows: &[Row]) {
    match ty {
        1 => {
            let m = u.get_search_albums();
            if let Some(vm) = m.as_any().downcast_ref::<VecModel<crate::Album>>() {
                for r in rows { vm.push(crate::Album {
                    id: r.id, name: r.name.as_str().into(), artist: r.sub.as_str().into(),
                    year: r.badge.as_str().into(), cover: crate::load::cover_now(&r.url),
                    from: crate::load::from(r.idx), to: crate::load::to(r.idx),
                }); }
            }
        }
        2 => {
            let m = u.get_search_playlists();
            if let Some(vm) = m.as_any().downcast_ref::<VecModel<crate::Playlist>>() {
                for r in rows { vm.push(crate::Playlist {
                    id: r.id, name: r.name.as_str().into(), plays: r.badge.as_str().into(),
                    cover: crate::load::cover_now(&r.url),
                    from: crate::load::from(r.idx), to: crate::load::to(r.idx),
                }); }
            }
        }
        3 => {
            let m = u.get_search_mvs();
            if let Some(vm) = m.as_any().downcast_ref::<VecModel<crate::MvItem>>() {
                for r in rows { vm.push(crate::MvItem {
                    id: r.id, name: r.name.as_str().into(), artist: r.sub.as_str().into(),
                    duration: r.badge.as_str().into(), plays: "".into(),
                    url: r.url.as_str().into(),
                    cover: crate::load::cover_now(&r.url),
                    from: crate::load::from(r.idx), to: crate::load::to(r.idx),
                }); }
            }
        }
        _ => {
            let m = u.get_search_radios();
            if let Some(vm) = m.as_any().downcast_ref::<VecModel<crate::Radio>>() {
                for r in rows { vm.push(crate::Radio {
                    id: r.id, name: r.name.as_str().into(), desc: r.sub.as_str().into(),
                    plays: r.badge.as_str().into(), cover: crate::load::cover_now(&r.url),
                    from: crate::load::from(r.idx), to: crate::load::to(r.idx),
                }); }
            }
        }
    }
}

/// 后台补齐封面后按全局索引回填（UI 线程 set_row_data）。
fn spawn_fill(ui_weak: slint::Weak<AppWindow>, ty: i32, rows: Vec<Row>) {
    let urls: Vec<(String, crate::persist::cache::CoverType)> = rows.iter()
        .filter(|r| !r.url.is_empty())
        .map(|r| (r.url.clone(), r.cover_type))
        .collect();
    if urls.is_empty() { return; }
    tokio::spawn(async move {
        let covers = crate::load::fetch_covers(&urls).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return };
            for r in &rows {
                let Some(Some(buf)) = covers.get(&r.url) else { continue };
                let img = slint::Image::from_rgba8(buf.clone());
                match ty {
                    1 => { let m = u.get_search_albums(); if let Some(mut x) = m.row_data(r.idx) { x.cover = img; m.set_row_data(r.idx, x); } }
                    2 => { let m = u.get_search_playlists(); if let Some(mut x) = m.row_data(r.idx) { x.cover = img; m.set_row_data(r.idx, x); } }
                    3 => { let m = u.get_search_mvs(); if let Some(mut x) = m.row_data(r.idx) { x.cover = img; m.set_row_data(r.idx, x); } }
                    _ => { let m = u.get_search_radios(); if let Some(mut x) = m.row_data(r.idx) { x.cover = img; m.set_row_data(r.idx, x); } }
                }
            }
        });
    });
}
