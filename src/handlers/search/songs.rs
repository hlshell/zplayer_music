//! 搜索回调：关键词搜索（分页/触底加载更多）+ 播放全部 + 按类型热搜占位。

use super::Ctx;
use crate::AppWindow;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// 搜索分页状态（跨类型共享）：新搜索时重置；触底加载更多时推进 offset。
pub(super) struct PageState {
    pub kw: String,
    pub ty: i32,
    pub offset: usize,
    pub has_more: bool,
    pub loading: bool,
}

pub(super) static PAGE: Mutex<Option<PageState>> = Mutex::new(None);

const SONG_PAGE: usize = 20;

/// 开始一页加载：校验状态并置 loading；返回 (kw, offset)。已在加载/无更多/关键词为空时返回 None。
pub(super) fn begin_page(ty: i32) -> Option<(String, usize)> {
    let mut g = PAGE.lock().ok()?;
    let st = st_mut(&mut *g);
    if st.ty != ty || st.loading || !st.has_more || st.kw.is_empty() {
        return None;
    }
    st.loading = true;
    Some((st.kw.clone(), st.offset))
}

fn st_mut(g: &mut Option<PageState>) -> &mut PageState {
    g.as_mut().expect("PAGE state must exist when loading")
}

/// 结束一页：推进 offset、记录 has_more、解除 loading。
pub(super) fn end_page(got: usize, has_more: bool) {
    if let Ok(mut g) = PAGE.lock() {
        if let Some(st) = g.as_mut() {
            st.offset += got;
            st.has_more = has_more && got > 0;
            st.loading = false;
        }
    }
}

/// 新搜索：重置分页状态。
fn reset_page(kw: String, ty: i32) {
    if let Ok(mut g) = PAGE.lock() {
        *g = Some(PageState { kw, ty, offset: 0, has_more: true, loading: false });
    }
}

/// 清空指定类型的搜索结果模型（新搜索时调用，配合骨架屏避免旧结果残留）。
fn clear_typed_model(u: &AppWindow, ty: i32) {
    match ty {
        0 => u.set_search_results(Rc::new(VecModel::<crate::Song>::default()).into()),
        1 => u.set_search_albums(Rc::new(VecModel::<crate::Album>::default()).into()),
        2 => u.set_search_playlists(Rc::new(VecModel::<crate::Playlist>::default()).into()),
        3 => u.set_search_mvs(Rc::new(VecModel::<crate::MvItem>::default()).into()),
        _ => u.set_search_radios(Rc::new(VecModel::<crate::Radio>::default()).into()),
    }
}

pub(crate) fn setup_songs(ctx: &Ctx) {
    // ---- 搜索：读取关键词 → 真实搜索结果 → 填充播放队列与搜索页 ----
    let ui_weak = ctx.ui.clone();
    let client_cb = ctx.client.clone();
    let rt_handle = ctx.rt.clone();
    let toast_cb = ctx.toast.clone();
    ctx.app.clone_strong().on_search_clicked(move || {
        let Some(u) = ui_weak.clone().upgrade() else { return; };
        let kw = u.get_search_text().to_string().trim().to_string();
        if kw.is_empty() { return; }
        // 进入搜索结果页 + 写入搜索历史（最新在前）并刷新主页历史 chips
        u.set_search_searched(true);
        crate::persist::history::record(&kw);
        push_history(&u);
        let ty = u.get_search_type();
        // 清空当前类型旧结果 + 置骨架屏 loading：新数据返回前不残留上一次内容
        clear_typed_model(&u, ty);
        u.set_search_loading(true);
        reset_page(kw.clone(), ty);
        let Some((kw, offset)) = begin_page(ty) else { return };
        if ty != 0 {
            // 非单曲类型：分派到分类搜索（专辑/歌单/MV/电台）
            super::typed::run(client_cb.clone(), rt_handle.clone(), ui_weak.clone(), toast_cb.clone(), kw, ty, offset as u32, false);
            return;
        }
        load_songs_page(client_cb.clone(), rt_handle.clone(), ui_weak.clone(), toast_cb.clone(), kw, offset, false);
    });

    // ---- 触底加载更多（所有类型共用） ----
    {
        let ui_weak = ctx.ui.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        let toast = ctx.toast.clone();
        ctx.app.clone_strong().on_search_load_more(move || {
            let Some(u) = ui_weak.clone().upgrade() else { return; };
            let ty = u.get_search_type();
            let Some((kw, offset)) = begin_page(ty) else { return };
            if ty == 0 {
                load_songs_page(client.clone(), rt.clone(), ui_weak.clone(), toast.clone(), kw, offset, true);
            } else {
                super::typed::run(client.clone(), rt.clone(), ui_weak.clone(), toast.clone(), kw, ty, offset as u32, true);
            }
        });
    }

    // ---- 搜索页"播放全部"：去重追加到播放列表，播放第一首 ----
    let ui_weak = ctx.ui.clone();
    let toast_sa = ctx.toast.clone();
    let rt_sa = ctx.rt.clone();
    ctx.app.clone_strong().on_search_play_all(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let m = u.get_search_results();
        let list: Vec<crate::Song> = (0..m.row_count()).filter_map(|i| m.row_data(i)).collect();
        if list.is_empty() {
            toast_sa.show("暂无搜索结果", "info");
            return;
        }
        let q = u.get_queue();
        let mut songs: Vec<crate::Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
        let mut have: std::collections::HashSet<i64> = songs.iter().map(|s| s.id as u32 as i64).collect();
        let mut added = 0usize;
        for s in &list {
            let xid = s.id as u32 as i64;
            if !have.insert(xid) { continue; }
            songs.push(s.clone());
            added += 1;
        }
        u.set_queue(Rc::new(VecModel::from(songs)).into());
        u.invoke_play_song(list[0].id);
        if added > 0 {
            toast_sa.show(&format!("已加入 {} 首歌曲到播放列表", added), "info");
        } else {
            toast_sa.show("歌曲已在播放列表", "info");
        }
        let urls: Vec<(String, crate::persist::cache::CoverType)> = list.iter()
            .map(|s| (s.url.to_string(), crate::persist::cache::CoverType::Song))
            .filter(|(x, _)| !x.is_empty())
            .collect();
        if !urls.is_empty() {
            let ui_p = ui_weak.clone();
            rt_sa.spawn(async move {
                let covers = crate::load::fetch_covers(&urls).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_p.upgrade() {
                        let q = u.get_queue();
                        for i in 0..q.row_count() {
                            let Some(s) = q.row_data(i) else { continue; };
                            let url = s.url.to_string();
                            if url.is_empty() { continue; }
                            if let Some(buf) = covers.get(&url).and_then(|x| x.as_ref()) {
                                let mut ns = s.clone();
                                ns.cover = slint::Image::from_rgba8(buf.clone());
                                q.set_row_data(i, ns);
                            }
                        }
                    }
                });
            });
        }
    });

    // ---- 搜索历史：单条删除 / 清空（持久化后刷新主页 chips）----
    {
        let ui_weak = ctx.ui.clone();
        ctx.app.clone_strong().on_search_history_remove(move |w| {
            crate::persist::history::remove(&w);
            if let Some(u) = ui_weak.upgrade() { push_history(&u); }
        });
        let ui_weak = ctx.ui.clone();
        ctx.app.clone_strong().on_search_history_clear(move || {
            crate::persist::history::clear();
            if let Some(u) = ui_weak.upgrade() { push_history(&u); }
        });
        // 启动时填充搜索历史
        if let Some(u) = ctx.ui.upgrade() { push_history(&u); }
    }

    // ---- 搜索框动态占位 + 搜索页热门词：随搜索类型切换，拉该类型最热资源名 ----
    {
        let ui_weak = ctx.ui.clone();
        let client = ctx.client.clone();
        let rt = ctx.rt.clone();
        ctx.app.clone_strong().on_search_type_changed(move |ty| {
            fetch_hot_words(client.clone(), rt.clone(), ui_weak.clone(), ty);
        });
        // 启动时按默认类型（单曲）拉一次
        fetch_hot_words(ctx.client.clone(), ctx.rt.clone(), ctx.ui.clone(), 0);
    }
}

/// 刷新搜索历史模型（搜索主页 chips 数据源）。
fn push_history(u: &AppWindow) {
    u.set_search_history(Rc::new(VecModel::from(
        crate::persist::history::list().iter().map(|w| w.as_str().into()).collect::<Vec<slint::SharedString>>()
    )).into());
}

/// 拉取指定类型的最热资源名列表：前 6 个填搜索页热门词，第 1 个填搜索框占位。
/// ty==0（单曲）时同时拉热搜榜详细（前 20 带 爆/↑ 角标）填搜索主页热搜列表。
fn fetch_hot_words(
    client: Arc<crate::network::netease::client::NeteaseClient>,
    rt: tokio::runtime::Handle,
    ui_weak: slint::Weak<AppWindow>,
    ty: i32,
) {
    rt.spawn(async move {
        let words = fetch_hot_word_list(&client, ty).await;
        let hot_items = if ty == 0 { fetch_hot_detail(&client).await } else { Vec::new() };
        if words.is_empty() && hot_items.is_empty() { return; }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_weak.upgrade() {
                if !hot_items.is_empty() {
                    u.set_search_hot_list(Rc::new(VecModel::from(hot_items)).into());
                }
                if !words.is_empty() {
                    u.set_search_placeholder(words[0].as_str().into());
                    u.set_hot_words(Rc::new(VecModel::from(
                        words.iter().map(|w| w.as_str().into()).collect::<Vec<slint::SharedString>>()
                    )).into());
                }
            }
        });
    });
}

/// 热搜榜详细（搜索主页用）：/search/hot/detail 前 20 条。
/// iconType 映射角标：1=爆（热门）5=↑（上升），其余无角标。
async fn fetch_hot_detail(client: &crate::network::netease::client::NeteaseClient) -> Vec<crate::HotItem> {
    let Ok(v) = client.get_plain("/search/hot/detail").await else { return Vec::new() };
    v.pointer("/data").and_then(|a| a.as_array())
        .map(|arr| arr.iter().take(20).filter_map(|m| {
            let word = m.get("searchWord").and_then(|w| w.as_str())?;
            if word.is_empty() { return None; }
            let tag = match m.get("iconType").and_then(|t| t.as_i64()).unwrap_or(0) {
                1 => "爆",
                5 => "↑",
                _ => "",
            };
            Some(crate::HotItem { word: word.into(), tag: tag.into() })
        }).collect())
        .unwrap_or_default()
}

/// 按类型取最热资源名列表（最多 6 个）：单曲=热搜词，专辑=最新专辑，歌单=精品歌单，MV=热门MV，电台=热门电台。
async fn fetch_hot_word_list(client: &crate::network::netease::client::NeteaseClient, ty: i32) -> Vec<String> {
    let v = match ty {
        1 => client.get_plain("/album/newest").await,
        2 => client.get_plain_query("/top/playlist/highquality", &[("limit", "6".into())]).await,
        3 => client.get_plain_query("/top/mv", &[("limit", "6".into())]).await,
        4 => client.get_plain_query("/dj/hot", &[("limit", "6".into())]).await,
        _ => client.get_plain("/search/hot/detail").await,
    };
    let Ok(v) = v else { return Vec::new() };
    let (arr_ptr, key) = match ty {
        1 => ("/albums", "name"),
        2 => ("/playlists", "name"),
        3 => ("/data", "name"),
        4 => ("/djRadios", "name"),
        _ => ("/data", "searchWord"),
    };
    v.pointer(arr_ptr).and_then(|a| a.as_array())
        .map(|arr| arr.iter().take(6)
            .filter_map(|m| m.get(key).and_then(|w| w.as_str()).map(|s| s.to_string()))
            .filter(|s| !s.is_empty())
            .collect())
        .unwrap_or_default()
}

/// 单曲搜索一页：offset 分页，append=true 时追加到现有模型。
fn load_songs_page(
    client: Arc<crate::network::netease::client::NeteaseClient>,
    rt: tokio::runtime::Handle,
    ui_weak: slint::Weak<AppWindow>,
    toast: Arc<super::super::ToastCtx>,
    kw: String,
    offset: usize,
    append: bool,
) {
    let weak2 = ui_weak.clone();
    let rt2 = rt.clone();
    let weak3 = ui_weak.clone();
    rt.spawn(async move {
        let raw: Vec<serde_json::Value> = match crate::network::service::search_songs(&client, &kw, SONG_PAGE as i64, offset as i64).await {
            Ok(v) => {
                let hm = v.pointer("/result/hasMore").and_then(|b| b.as_bool()).unwrap_or(false);
                let arr = v.pointer("/result/songs").and_then(|a| a.as_array()).cloned().unwrap_or_default();
                let n = arr.len();
                end_page(n, hm);
                arr
            }
            Err(_) => {
                end_page(0, false);
                Vec::new()
            }
        };
        let raw = super::super::player::ensure_covers(&client, raw).await;
        let items = crate::load::parse_tracks(&raw);
        let page_items = items.clone();
        let toast2 = toast.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = weak2.upgrade() {
                let songs: Vec<crate::Song> = items.into_iter().map(|d| crate::Song {
                    id: d.id, name: d.name, artist: d.artist, album: d.album,
                    cover: crate::load::cover_now(&d.url),
                    duration: d.duration, duration_sec: d.duration_sec,
                    url: d.url.into(),
                    liked: crate::persist::liked::is_liked(d.id as u32 as i64),
                }).collect();
                if append {
                    // 追加：底层模型是 VecModel，downcast 后 push（自动通知视图）
                    let model = u.get_search_results();
                    if let Some(vm) = model.as_any().downcast_ref::<VecModel<crate::Song>>() {
                        for s in songs { vm.push(s); }
                    }
                } else {
                    u.set_search_results(Rc::new(VecModel::from(songs)).into());
                }
                // 数据到达（含失败空页），撤下骨架屏
                u.set_search_loading(false);
                let total = u.get_search_results().row_count();
                if !append {
                    if total > 0 {
                        toast2.show(&format!("搜索到 {} 首歌曲", total), "info");
                    } else {
                        toast2.show("未找到相关歌曲", "error");
                    }
                }
            }
        });
        let search_urls: Vec<(String, crate::persist::cache::CoverType)> = page_items.iter()
            .map(|d| (d.url.clone(), crate::persist::cache::CoverType::Song))
            .filter(|(u, _)| !u.is_empty())
            .collect();
        if !search_urls.is_empty() {
            rt2.spawn(async move {
                let covers = crate::load::fetch_covers(&search_urls).await;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = weak3.clone().upgrade() {
                        let model = u.get_search_results();
                        for i in 0..model.row_count() {
                            let Some(s) = model.row_data(i) else { continue; };
                            let url = s.url.to_string();
                            if url.is_empty() { continue; }
                            if let Some(buf) = covers.get(url.as_str()).and_then(|x| x.as_ref()) {
                                let mut ns = s.clone();
                                ns.cover = slint::Image::from_rgba8(buf.clone());
                                model.set_row_data(i, ns);
                            }
                        }
                        // 当前曲若匹配也刷新
                        let cur = u.get_current_song();
                        if let Some(buf) = covers.get(cur.url.to_string().as_str()).and_then(|x| x.as_ref()) {
                            let mut nc = cur.clone();
                            nc.cover = slint::Image::from_rgba8(buf.clone());
                            u.set_current_song(nc);
                        }
                    }
                });
            });
        }
    });
}
