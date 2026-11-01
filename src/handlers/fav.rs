//! 我的最爱 / 最近播放：页面数据填充与行内操作回调。
//! 填充时机：侧边栏切入对应页（page-fill）+ 收藏/删除操作后按当前页条件刷新；
//! 封面统一走 cover_now 建占位 + warm_rows 逐行回填（未命中后台补齐，列表零网络阻塞）。

use super::Ctx;
use crate::persist::{cache::CoverType, liked, recent};
use crate::{AppWindow, LikedRes, Song};
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

/// 歌曲模型：把封面为空但 URL 有效的行交给后台补齐，就绪后 set_row_data 回填。
fn warm_song_rows(model: &slint::ModelRc<Song>) {
    let rows: Vec<(usize, String, CoverType)> = (0..model.row_count())
        .filter_map(|i| {
            let s = model.row_data(i)?;
            if s.cover.size().width > 0 || s.url.is_empty() {
                return None;
            }
            Some((i, s.url.to_string(), CoverType::Song))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let m = model.clone();
    crate::load::warm_rows(
        rows,
        Rc::new(move |i, img| {
            if let Some(mut s) = m.row_data(i) {
                s.cover = img;
                m.set_row_data(i, s);
            }
        }),
    );
}

/// 资源模型：同上，按资源类型子目录取图。
fn warm_res_rows(model: &slint::ModelRc<LikedRes>, ct: CoverType) {
    let rows: Vec<(usize, String, CoverType)> = (0..model.row_count())
        .filter_map(|i| {
            let r = model.row_data(i)?;
            if r.cover_img.size().width > 0 || r.cover.is_empty() {
                return None;
            }
            Some((i, r.cover.to_string(), ct))
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let m = model.clone();
    crate::load::warm_rows(
        rows,
        Rc::new(move |i, img| {
            if let Some(mut r) = m.row_data(i) {
                r.cover_img = img;
                m.set_row_data(i, r);
            }
        }),
    );
}

/// 填充"我的最爱"5 分类列表（歌曲 + 歌单/专辑/MV/电台资源）。
pub(crate) fn fill_liked(u: &AppWindow) {
    let songs: Vec<Song> = liked::list_songs()
        .into_iter()
        .map(|s| Song {
            id: s.id as i32,
            name: s.name.into(),
            artist: s.artist.into(),
            album: s.album.into(),
            cover: crate::load::cover_now(&s.cover),
            duration: s.duration.into(),
            duration_sec: s.duration_sec,
            url: s.cover.into(),
            liked: true,
        })
        .collect();
    u.set_liked_song_list(Rc::new(VecModel::from(songs)).into());
    warm_song_rows(&u.get_liked_song_list());

    let build = |kind: i32| {
        let v: Vec<LikedRes> = liked::list_resources_by_kind(kind)
            .into_iter()
            .map(|r| LikedRes {
                kind: r.kind,
                // 资源 id 可能超过 32 位：登记真实 id，点击进入详情时再还原
                id: crate::core::ids::remember(kind, r.id),
                name: r.name.clone().into(),
                sub: r.sub.clone().into(),
                cover: r.cover.clone().into(),
                ts: r.ts as i32,
                cover_img: crate::load::cover_now(&r.cover),
            })
            .collect();
        slint::ModelRc::from(Rc::new(VecModel::from(v)))
    };
    let playlists = build(0);
    let albums = build(1);
    let mvs = build(4);
    let radios = build(3);
    u.set_liked_playlists(playlists.clone());
    u.set_liked_albums(albums.clone());
    u.set_liked_mvs(mvs.clone());
    u.set_liked_radios(radios.clone());
    warm_res_rows(&playlists, CoverType::Playlist);
    warm_res_rows(&albums, CoverType::Album);
    warm_res_rows(&mvs, CoverType::Mv);
    warm_res_rows(&radios, CoverType::Radio);
}

/// 填充"最近播放"列表（最新在前，按 id 去重兜底）。
/// 同时填充歌曲（kind=0）与 MV（kind=4）两个分类。
pub(crate) fn fill_recent(u: &AppWindow) {
    let mut seen = std::collections::HashSet::new();
    let songs: Vec<Song> = recent::list_by_kind(0)
        .into_iter()
        .filter(|e| seen.insert(e.id))
        .map(|e| Song {
            id: e.id as i32,
            name: e.name.into(),
            artist: e.artist.into(),
            album: e.album.into(),
            cover: crate::load::cover_now(&e.cover),
            duration: e.duration.into(),
            duration_sec: e.duration_sec,
            url: e.cover.clone().into(),
            liked: liked::is_liked(e.id),
        })
        .collect();
    u.set_recent_song_list(Rc::new(VecModel::from(songs)).into());
    warm_song_rows(&u.get_recent_song_list());

    // MV 分类：用 Song 模型承载（id 用 ids::remember 登记，便于 open-resource-detail(4,id) 还原）
    let mvs: Vec<Song> = recent::list_by_kind(4)
        .into_iter()
        .filter(|e| seen.insert(e.id))
        .map(|e| Song {
            id: crate::core::ids::remember(4, e.id) as i32,
            name: e.name.into(),
            artist: e.artist.into(),
            album: e.album.into(),
            cover: crate::load::cover_now(&e.cover),
            duration: e.duration.into(),
            duration_sec: e.duration_sec,
            url: e.cover.clone().into(),
            liked: false,
        })
        .collect();
    u.set_recent_mv_list(Rc::new(VecModel::from(mvs)).into());
    warm_song_rows(&u.get_recent_mv_list());
}

pub fn setup(ctx: &Ctx) {
    // ---- 页面切入填充：7=我的最爱 8=最近播放（appwindow changed current-nav 触发）----
    let ui_weak = ctx.ui.clone();
    let client_pf = ctx.client.clone();
    let rt_pf = ctx.rt.clone();
    let ui_pf = ctx.ui.clone();
    ctx.app.clone_strong().on_page_fill(move |nav| {
        let Some(u) = ui_weak.upgrade() else { return; };
        match nav {
            7 => {
                fill_liked(&u);
                // v1 旧收藏（仅 id 无元数据）：后台 song_detail 批量补全一次，完成后回写并刷新
                let missing = liked::ids_without_meta();
                if !missing.is_empty() {
                    let client = client_pf.clone();
                    let ui2 = ui_pf.clone();
                    rt_pf.spawn(async move {
                        if let Ok(v) = crate::network::service::song_detail(&client, &missing).await {
                            let arr = v.get("songs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                            let base = crate::load::parse_tracks(&arr);
                            let entries: Vec<liked::LikedSong> = base
                                .iter()
                                .map(|d| liked::LikedSong {
                                    id: d.id as u32 as i64,
                                    name: d.name.to_string(),
                                    artist: d.artist.to_string(),
                                    album: d.album.to_string(),
                                    cover: d.url.clone(),
                                    duration: d.duration.to_string(),
                                    duration_sec: d.duration_sec,
                                    ts: 0,
                                })
                                .collect();
                            if !entries.is_empty() {
                                log::info!("[liked] v1 收藏元数据补全 {} 首", entries.len());
                                liked::update_song_meta(entries);
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(u) = ui2.upgrade() {
                                        if u.get_current_nav() == 7 {
                                            fill_liked(&u);
                                        }
                                    }
                                });
                            }
                        }
                    });
                }
            }
            8 => fill_recent(&u),
            _ => {}
        }
    });

    // ---- 我的最爱：行内红心取消资源收藏 ----
    let ui_weak = ctx.ui.clone();
    let toast_un = ctx.toast.clone();
    ctx.app.clone_strong().on_liked_unlike(move |kind, id| {
        let Some(u) = ui_weak.upgrade() else { return; };
        let uid = crate::core::ids::resolve(kind, id);
        if uid == 0 { return; }
        // 已收藏 → toggle 即删除（name/sub/cover 仅收藏分支使用，此处传空）
        liked::toggle_resource(kind, uid, String::new(), String::new(), String::new());
        toast_un.show("已取消收藏", "info");
        fill_liked(&u);
        // 详情页若正开着同一资源，同步头部爱心状态
        if u.get_detail_res_kind() == kind && u.get_detail_res_id() == id {
            u.set_detail_liked(false);
        }
    });

    // ---- 最近播放：单条删除 ----
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_recent_remove(move |id| {
        let Some(u) = ui_weak.upgrade() else { return; };
        // MV 行的 id 是 ids::remember(4, real_id) 登记的索引，需还原；歌曲行是真实 id。
        let real_id = {
            let resolved = crate::core::ids::resolve(4, id);
            if resolved > 0 { resolved } else { id as u32 as i64 }
        };
        recent::remove(real_id);
        fill_recent(&u);
    });

    // ---- 最近播放：清空记录 ----
    let ui_weak = ctx.ui.clone();
    let toast_cl = ctx.toast.clone();
    ctx.app.clone_strong().on_recent_clear(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        recent::clear();
        fill_recent(&u);
        toast_cl.show("已清空最近播放", "info");
    });

    // ---- 最近播放：分类切换（歌曲/MV）。数据已预填，仅同步 tab 状态 ----
    ctx.app.clone_strong().on_recent_tab_changed(move |_idx| {
        // 两个分类的列表均已在 fill_recent 中填充，切换仅由 UI 层 tab 属性驱动，
        // 无需 Rust 侧重新拉取。
    });
}
