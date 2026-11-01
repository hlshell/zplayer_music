//! 收藏回调：当前曲收藏切换、指定 id 收藏切换、详情页资源收藏切换。

use super::Ctx;
use crate::persist::liked;
use slint::{ComponentHandle, Model};

pub(crate) fn setup_like(ctx: &Ctx) {
    // ---- 详情页头部资源收藏切换（本地持久化 + 快照；无网络 API）----
    let ui_weak = ctx.ui.clone();
    let toast_dlike = ctx.toast.clone();
    ctx.app.clone_strong().on_detail_like_toggle(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let kind = u.get_detail_res_kind();
        if kind < 0 { return; }
        let id = crate::core::ids::resolve(kind, u.get_detail_res_id());
        if id == 0 { return; }
        let name = u.get_detail_title().to_string();
        let sub = u.get_detail_desc().to_string();
        let cover = u.get_detail_res_cover().to_string();
        let now = liked::toggle_resource(kind, id, name, sub, cover);
        u.set_detail_liked(now);
        toast_dlike.show(if now { "已收藏到我的最爱" } else { "已取消收藏" }, "info");
        if u.get_current_nav() == 7 {
            super::super::fav::fill_liked(&u);
        }
    });

    // ---- 收藏切换：立即更新本地集合并持久化 + 同步 UI；后台调 like_song API ----
    let ui_weak = ctx.ui.clone();
    let client_like = ctx.client.clone();
    let rt_like = ctx.rt.clone();
    ctx.app.clone_strong().on_like_toggle(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let cur = u.get_current_song();
        let uid = cur.id as u32 as i64;
        if uid == 0 { return; }
        // 收藏快照：从我的最爱页重启后零网络展示
        let meta = liked::LikedSong {
            id: uid,
            name: cur.name.to_string(),
            artist: cur.artist.to_string(),
            album: cur.album.to_string(),
            cover: cur.url.to_string(),
            duration: cur.duration.to_string(),
            duration_sec: cur.duration_sec,
            ts: 0,
        };
        let now_liked = liked::toggle_liked(uid, Some(meta));
        u.set_liked(now_liked);
        let client_c = client_like.clone();
        rt_like.spawn(async move {
            let _ = client_c.like_song(uid as u64, now_liked).await;
        });
        let q = u.get_queue();
        for i in 0..q.row_count() {
            if let Some(s) = q.row_data(i) {
                if s.id == u.get_current_song().id {
                    let mut ns = s.clone();
                    ns.liked = now_liked;
                    q.set_row_data(i, ns);
                    break;
                }
            }
        }
        if u.get_current_nav() == 7 {
            super::super::fav::fill_liked(&u);
        }
    });

    let ui_weak = ctx.ui.clone();
    let client_like2 = ctx.client.clone();
    let rt_like2 = ctx.rt.clone();
    ctx.app.clone_strong().on_like_toggle_id(move |id| {
        let Some(u) = ui_weak.upgrade() else { return; };
        let uid = id as u32 as i64;
        if uid == 0 { return; }
        // 元数据快照：依次从队列 / 详情页列表 / 我的最爱列表查找该 id 的行数据
        let mut meta: Option<liked::LikedSong> = None;
        for model in [u.get_queue(), u.get_detail_songs(), u.get_liked_song_list()] {
            for i in 0..model.row_count() {
                let Some(s) = model.row_data(i) else { continue; };
                if s.id == id {
                    meta = Some(liked::LikedSong {
                        id: uid,
                        name: s.name.to_string(),
                        artist: s.artist.to_string(),
                        album: s.album.to_string(),
                        cover: s.url.to_string(),
                        duration: s.duration.to_string(),
                        duration_sec: s.duration_sec,
                        ts: 0,
                    });
                    break;
                }
            }
            if meta.is_some() { break; }
        }
        let now_liked = liked::toggle_liked(uid, meta);
        let client_c = client_like2.clone();
        rt_like2.spawn(async move {
            let _ = client_c.like_song(uid as u64, now_liked).await;
        });
        if u.get_current_song().id == id {
            u.set_liked(now_liked);
        }
        let q = u.get_queue();
        for i in 0..q.row_count() {
            if let Some(s) = q.row_data(i) {
                if s.id == id {
                    let mut ns = s.clone();
                    ns.liked = now_liked;
                    q.set_row_data(i, ns);
                    break;
                }
            }
        }
        if u.get_current_nav() == 7 {
            super::super::fav::fill_liked(&u);
        }
    });
}
