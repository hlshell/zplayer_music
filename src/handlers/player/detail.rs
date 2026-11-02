//! 详情页回调：每日精选 / 歌单 / 专辑 / 歌手 / 电台 / MV 打开，
//! 详情搜索本地过滤，详情返回动画。

use super::{ensure_covers, Ctx};
use crate::persist::liked;
use crate::Song;
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;
use std::time::Duration;

/// 业务码：网易云响应 code（缺失视为 200 成功）
fn biz_code(v: &serde_json::Value) -> i64 {
    v.get("code").and_then(|c| c.as_i64()).unwrap_or(200)
}

/// 业务错误文案：优先 msg / message，兜底通用说明
fn biz_msg(v: &serde_json::Value) -> String {
    v.get("msg").or_else(|| v.get("message")).and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("资源不存在或已下架").to_string()
}

/// 把网络层英文错误转成面向用户的提示（详情页不直接显示 reqwest 原文）
fn friendly_err(msg: &str) -> String {
    if msg.starts_with("request failed:") || msg.contains("error sending request") {
        "网络连接失败，请检查网络后重试".to_string()
    } else if msg.starts_with("HTTP 5") {
        "服务暂时不可用，请稍后重试".to_string()
    } else {
        msg.to_string()
    }
}

pub(crate) fn setup_detail(ctx: &Ctx) {
    // ---- 详情搜索本地过滤：detail-all-songs 为全量（含封面），输入过滤后写回 detail-songs ----
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_detail_search_changed(move |t| {
        let Some(u) = ui_weak.upgrade() else { return; };
        let kw = t.trim().to_lowercase();
        let all = u.get_detail_all_songs();
        let filtered: Vec<Song> = if kw.is_empty() {
            (0..all.row_count()).filter_map(|i| all.row_data(i)).collect()
        } else {
            (0..all.row_count())
                .filter_map(|i| all.row_data(i))
                .filter(|s| {
                    s.name.to_lowercase().contains(&kw) || s.artist.to_lowercase().contains(&kw)
                })
                .collect()
        };
        u.set_detail_songs(Rc::new(VecModel::from(filtered)).into());
    });

    // ---- 详情返回：向左滑出淡出动画后卸载（避免直接置 visible=false 造成生硬/卡顿）----
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_detail_back(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        u.set_detail_fade(false);
        u.set_detail_shift(-140.0);
        let uw = ui_weak.clone();
        slint::Timer::single_shot(std::time::Duration::from_millis(300), move || {
            if let Some(u) = uw.upgrade() {
                u.set_detail_visible(false);
                // 复位，供下次打开重新淡入
                u.set_detail_fade(true);
                u.set_detail_shift(0.0);
            }
        });
    });

    // ---- 点击 Hero 区域：打开每日精选统一详情页（覆盖层，不切 current-nav）----
    // 每日推荐 res-kind=-1（头部爱心隐藏），不进入资源收藏
    let ui_weak = ctx.ui.clone();
    let rt_detail = ctx.rt.clone();
    ctx.app.clone_strong().on_open_daily_detail(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let pool = crate::load::HERO_POOL.lock().unwrap().clone();
        u.set_detail_title("每日为你精选".into());
        u.set_detail_desc("每天更新的精选歌曲清单，点击歌曲立即播放".into());
        u.set_detail_type("每日推荐".into());
        u.set_detail_cover(u.get_hero_cover());
        u.set_detail_search("".into());
        // 每日推荐无资源收藏
        u.set_detail_res_kind(-1);
        u.set_detail_res_id(0);
        u.set_detail_res_cover("".into());
        u.set_detail_liked(false);
        let songs: Vec<Song> = pool.iter().map(|d| Song {
            id: d.id,
            name: d.name.clone(),
            artist: d.artist.clone(),
            album: d.album.clone(),
            cover: crate::load::cover_now(&d.url),
            duration: d.duration.clone(),
            duration_sec: d.duration_sec,
            url: d.url.clone().into(),
            liked: liked::is_liked(d.id as u32 as i64),
        }).collect();
        // 同一份模型挂到两个属性：全量源与显示列表初始等值，
        // 共享 ModelRc 避免把整页歌曲复制两遍（详情页内存/构造各减半）。
        let model: Rc<VecModel<Song>> = Rc::new(VecModel::from(songs));
        u.set_detail_all_songs(model.clone().into());
        u.set_detail_songs(model.into());
        u.set_detail_visible(true);
        // 从左滑入淡入动画：先置 fade=false + shift=-140px，稍后回正触发动画
        u.set_detail_fade(false);
        u.set_detail_shift(-140.0);
        let uw_anim = ui_weak.clone();
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            if let Some(u) = uw_anim.upgrade() {
                u.set_detail_fade(true);
                u.set_detail_shift(0.0);
            }
        });
        let ui_p = ui_weak.clone();
        rt_detail.spawn(async move {
            let urls: Vec<(String, crate::persist::cache::CoverType)> =
                pool.iter().map(|d| (d.url.clone(), crate::persist::cache::CoverType::Song)).collect();
            let covers = crate::load::fetch_covers(&urls).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_p.upgrade() {
                    let m = u.get_detail_all_songs();
                    for i in 0..m.row_count() {
                        let Some(s) = m.row_data(i) else { continue; };
                        let url = s.url.to_string();
                        if url.is_empty() { continue; }
                        if let Some(buf) = covers.get(&url).and_then(|x| x.as_ref()) {
                            let mut ns = s.clone();
                            ns.cover = slint::Image::from_rgba8(buf.clone());
                            m.set_row_data(i, ns);
                        }
                    }
                    // 若当前无搜索词，同步展示模型封面
                    if u.get_detail_search().trim().is_empty() {
                        let d = u.get_detail_songs();
                        for i in 0..d.row_count() {
                            let Some(s) = d.row_data(i) else { continue; };
                            if s.cover.size().width == 0 {
                                if let Some(all_s) = (0..u.get_detail_all_songs().row_count())
                                    .find_map(|j| u.get_detail_all_songs().row_data(j).filter(|a| a.id == s.id))
                                {
                                    let mut ns = s.clone();
                                    ns.cover = all_s.cover;
                                    d.set_row_data(i, ns);
                                }
                            }
                        }
                    }
                }
            });
        });
    });

    // ---- 首页歌单/专辑/歌手卡片：打开统一详情页（覆盖层，不切 current-nav）----
    // kind: 0=歌单 1=专辑 2=歌手 3=电台 4=MV；id 为 Slint int（i32），大 ID 经 ids 表还原
    let ui_weak = ctx.ui.clone();
    let rt_res = ctx.rt.clone();
    let client_res = ctx.client.clone();
    let toast_res = ctx.toast.clone();
    ctx.app.clone_strong().on_open_resource_detail(move |kind, id| {
        let Some(u) = ui_weak.upgrade() else { return; };
        // ===== MV（kind=4）：走全屏播放器，不进详情页 =====
        if kind == 4 {
            let mv_id = crate::core::ids::resolve(kind, id);
            let nav = u.get_current_nav();
            let ui4 = ui_weak.clone();
            let client4 = client_res.clone();
            let rt4 = rt_res.clone();

            // 根据入口来源预置队列：
            // - 收藏夹(nav=7)：队列=收藏的全部 MV，从点击的那首开始
            // - 搜索页(nav=1)：把点击的 MV 插入到正在播放队列的下一位（不替换/清空原队列）
            // - MV页(nav=4)：队列已由 browse.rs sync_mv_queue 维护
            // - 其他：open_mv_player 内部兜底为单曲
            if nav == 1 {
                let mvs = u.get_search_mvs();
                if let Some(m) = (0..mvs.row_count())
                    .filter_map(|i| mvs.row_data(i))
                    .find(|m| crate::core::ids::resolve(4, m.id) == mv_id)
                {
                    let item = crate::handlers::mv::queue::MvQueueItem {
                        id: mv_id,
                        name: m.name.to_string(),
                        artist: m.artist.to_string(),
                        cover_url: m.url.to_string(),
                        duration: m.duration.to_string(),
                        from: 0xFF333333,
                        to: 0xFF222222,
                    };
                    if let Ok(mut q) = crate::handlers::mv::queue::queue().lock() {
                        q.insert_next(item);
                    }
                }
            }
            if nav == 7 {
                let favs = crate::persist::liked::list_resources_by_kind(4);
                let qitems: Vec<crate::handlers::mv::queue::MvQueueItem> = favs.iter().map(|r| {
                    crate::handlers::mv::queue::MvQueueItem {
                        id: r.id,
                        name: r.name.clone(),
                        artist: r.sub.clone(),
                        cover_url: r.cover.clone(),
                        duration: String::new(),
                        from: 0xFF333333,
                        to: 0xFF222222,
                    }
                }).collect();
                let start_idx = qitems.iter().position(|x| x.id == mv_id).unwrap_or(0);
                if let Ok(mut q) = crate::handlers::mv::queue::queue().lock() {
                    q.set_items(qitems, start_idx);
                }
            }

            // 先调 mv_detail 拿标题/艺术家（MV 卡片点了直接进全屏播放器，需要标题）
            rt_res.spawn(async move {
                let (title, artist) = match crate::network::service::mv_detail(&client4, mv_id as u64).await {
                    Ok(v) => {
                        if biz_code(&v) == 200 {
                            let d = v.get("data").filter(|x| !x.is_null());
                            let t = d.and_then(|d| d.get("name")).and_then(|n| n.as_str()).unwrap_or("MV").to_string();
                            let a = d.and_then(|d| d.get("artistName")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                            (t, a)
                        } else {
                            (String::from("MV"), String::new())
                        }
                    }
                    Err(_) => (String::from("MV"), String::new()),
                };
                crate::handlers::mv::player::open_mv_player_from_ids(&ui4, &client4, &rt4, mv_id as i64, title, artist);
            });
            return;
        }
        // 资源 id 还原：Slint int 为 i32，超过 32 位的 id 已在列表构建时登记进 ids 表
        let rid = crate::core::ids::resolve(kind, id);
        let type_label = match kind { 0 => "歌单", 1 => "专辑", 2 => "歌手", 3 => "电台", 4 => "MV", _ => "资源" };
        u.set_detail_type(type_label.into());
        u.set_detail_visible(true);
        // 收藏标识：kind 与详情一致；歌手(kind=2)不支持收藏，隐藏爱心
        let res_kind = if kind == 2 { -1 } else { kind };
        u.set_detail_res_kind(res_kind);
        u.set_detail_res_id(id);
        u.set_detail_res_cover("".into()); // 拉到 head_cover 后回填
        u.set_detail_liked(if res_kind >= 0 { liked::is_resource_liked(res_kind, rid) } else { false });
        // 从左滑入淡入动画：先置 fade=false + shift=-140px，稍后回正触发动画
        u.set_detail_fade(false);
        u.set_detail_shift(-140.0);
        let uw_anim = ui_weak.clone();
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            if let Some(u) = uw_anim.upgrade() {
                u.set_detail_fade(true);
                u.set_detail_shift(0.0);
            }
        });
        u.set_detail_search("".into());
        u.set_detail_title("加载中…".into());
        u.set_detail_desc("".into());
        u.set_detail_songs(Rc::new(VecModel::<Song>::from(Vec::new())).into());
        u.set_detail_all_songs(Rc::new(VecModel::<Song>::from(Vec::new())).into());
        u.set_detail_cover(slint::Image::default());
        let ui_p = ui_weak.clone();
        let client_call = client_res.clone();
        let toast = toast_res.clone();
        rt_res.spawn(async move {
            let got: Result<(String, String, String, Vec<crate::load::SongBase>), String> = match kind {
                0 => match crate::network::service::playlist_detail(&client_call, rid as u64, 1000).await {
                    Ok(v) => {
                        if biz_code(&v) != 200 {
                            Err(biz_msg(&v))
                        } else if let Some(p) = v.get("playlist").filter(|x| !x.is_null()) {
                            let title = p.get("name").and_then(|n| n.as_str()).unwrap_or("歌单").to_string();
                            let desc = p.get("description").and_then(|n| n.as_str()).unwrap_or("")
                                .chars().take(60).collect::<String>();
                            let cover = p.get("coverImgUrl").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let mut tracks = p.get("tracks").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                            // 兜底：v6 的 s 参数限制 tracks 返回数（s=8 仅 10 首），trackIds 才是全量
                            let total_ids: Vec<i64> = p.get("trackIds")
                                .and_then(|t| t.as_array()).map(|a| a.iter()
                                    .filter_map(|x| x.get("id").and_then(|i| i.as_i64())).take(500).collect())
                                .unwrap_or_default();
                            if tracks.len() < total_ids.len() && !total_ids.is_empty() {
                                if let Ok(dv) = crate::network::service::song_detail(&client_call, &total_ids).await {
                                    let full = dv.get("songs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                                    if !full.is_empty() {
                                        log::info!("[detail] 歌单{} trackIds 兜底: {} -> {} 首", rid, tracks.len(), full.len());
                                        tracks = full;
                                    }
                                }
                            }
                            Ok((title, desc, cover, crate::load::parse_tracks(&tracks)))
                        } else {
                            Err("歌单数据为空".to_string())
                        }
                    }
                    Err(e) => Err(e),
                },
                1 => match crate::network::service::album_songs(&client_call, rid as u64).await {
                    Ok(v) => {
                        if biz_code(&v) != 200 {
                            Err(biz_msg(&v))
                        } else if let Some(al) = v.get("album").filter(|x| !x.is_null()) {
                            let title = al.get("name").and_then(|n| n.as_str()).unwrap_or("专辑").to_string();
                            let desc = al.get("artist").and_then(|x| x.get("name")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let cover = al.get("picUrl").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let songs = v.get("songs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
                            let songs = ensure_covers(&client_call, songs).await;
                            Ok((title, desc, cover, crate::load::parse_tracks(&songs)))
                        } else {
                            Err("专辑数据为空".to_string())
                        }
                    }
                    Err(e) => Err(e),
                },
                2 => match crate::network::service::artist_songs(&client_call, rid as u64, 50).await {
                    Ok(v) => {
                        if biz_code(&v) != 200 {
                            Err(biz_msg(&v))
                        } else {
                            // 兼容多种响应形状：data.songs（v1/artist/songs）、songs、hotSongs
                            let songs_v = v.pointer("/data/songs")
                                .or_else(|| v.get("songs"))
                                .or_else(|| v.get("hotSongs"))
                                .and_then(|t| t.as_array()).cloned().unwrap_or_default();
                            let songs_v = ensure_covers(&client_call, songs_v).await;
                            let base = crate::load::parse_tracks(&songs_v);
                            if base.is_empty() {
                                Err("该歌手暂无可用歌曲".to_string())
                            } else {
                                let title = base.first().map(|d| d.artist.to_string())
                                    .filter(|s| !s.is_empty()).unwrap_or_else(|| "歌手".to_string());
                                Ok((title, "热门歌曲".to_string(), String::new(), base))
                            }
                        }
                    }
                    Err(e) => Err(e),
                },
                3 => {
                    // 电台详情：/weapi/dj/program、/weapi/dj/detail 已被服务端封锁（404），
                    // 改为读取浏览页 (browse.rs) 已拉取的个性化推荐节目池（按电台 id 分组）。
                    let pool = crate::handlers::browse::RADIO_PROGRAMS.lock().unwrap();
                    let prog: Vec<serde_json::Value> = pool.iter()
                        .find(|(prid, _)| *prid == rid)
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    drop(pool);
                    if prog.is_empty() {
                        Err("该电台暂无节目数据，请刷新电台页后重试".to_string())
                    } else {
                        let radio_meta = prog.first().and_then(|p| p.get("radio"));
                        let title = radio_meta.and_then(|r| r.get("name")).and_then(|n| n.as_str())
                            .or_else(|| prog.first().and_then(|p| p.get("name")).and_then(|n| n.as_str()))
                            .unwrap_or("电台").to_string();
                        let desc = radio_meta.and_then(|r| r.get("desc")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                        let cover = radio_meta.and_then(|r| r.get("picUrl")).and_then(|n| n.as_str()).unwrap_or("").to_string();
                        // 该电台的节目 mainSong 即音频
                        let songs: Vec<serde_json::Value> =
                            prog.iter().filter_map(|p| p.get("mainSong").cloned()).collect();
                        Ok((title, desc, cover, crate::load::parse_tracks(&songs)))
                    }
                },
                4 => match crate::network::service::mv_detail(&client_call, rid as u64).await {
                    Ok(v) => {
                        if biz_code(&v) != 200 {
                            Err(biz_msg(&v))
                        } else if let Some(d) = v.get("data").filter(|x| !x.is_null()) {
                            let title = d.get("name").and_then(|n| n.as_str()).unwrap_or("MV").to_string();
                            let artist = d.get("artistName").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let desc = d.get("desc").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let cover = d.get("cover").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let full_desc = if desc.is_empty() { artist } else if artist.is_empty() { desc } else { format!("{} · {}", artist, desc) };
                            Ok((title, full_desc, cover, Vec::new()))
                        } else {
                            Err("MV 数据为空".to_string())
                        }
                    }
                    Err(e) => Err(e),
                },
                _ => Err("暂不支持该类型".to_string()),
            };
            let (title, desc, cover_url, base) = match got {
                Ok(x) => x,
                Err(msg) => {
                    // 失败态：给出明确原因（不再静默退化为"歌单/专辑"通用标题 + 空列表）
                    let msg = friendly_err(&msg);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u) = ui_p.upgrade() {
                            u.set_detail_title("加载失败".into());
                            u.set_detail_desc(msg.clone().into());
                            u.set_detail_res_kind(-1);   // 失败资源不提供收藏
                            u.set_detail_res_cover("".into());
                            u.set_detail_cover(slint::Image::default());
                            u.set_detail_songs(Rc::new(VecModel::<Song>::from(Vec::new())).into());
                            u.set_detail_all_songs(Rc::new(VecModel::<Song>::from(Vec::new())).into());
                            toast.show(&msg, "error");
                        }
                    });
                    return;
                }
            };
            let head_type = match kind {
                0 => crate::persist::cache::CoverType::Playlist,
                1 => crate::persist::cache::CoverType::Album,
                2 => crate::persist::cache::CoverType::Artist,
                3 => crate::persist::cache::CoverType::Radio,
                _ => crate::persist::cache::CoverType::Mv,
            };
            let head_cover = if cover_url.is_empty() {
                base.first().map(|d| d.url.clone()).unwrap_or_default()
            } else { cover_url };
            let head_for_ui = head_cover.clone();
            let mut urls: Vec<(String, crate::persist::cache::CoverType)> =
                base.iter().map(|d| (d.url.clone(), crate::persist::cache::CoverType::Song)).collect();
            urls.push((head_cover.clone(), head_type));
            let ui_c = ui_p.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_p.upgrade() {
                    u.set_detail_title(title.into());
                    u.set_detail_desc(desc.into());
                    u.set_detail_cover(crate::load::cover_now(&head_for_ui));
                    // 头图 URL 快照（收藏资源时写入）
                    u.set_detail_res_cover(head_for_ui.clone().into());
                    let songs: Vec<Song> = base.iter().map(|d| Song {
                        id: d.id,
                        name: d.name.clone(),
                        artist: d.artist.clone(),
                        album: d.album.clone(),
                        cover: crate::load::cover_now(&d.url),
                        duration: d.duration.clone(),
                        duration_sec: d.duration_sec,
                        url: d.url.clone().into(),
                        liked: liked::is_liked(d.id as u32 as i64),
                    }).collect();
                    // 同 76-80 行：两个属性共享同一模型，避免整页歌曲复制两遍
                    let model: Rc<VecModel<Song>> = Rc::new(VecModel::from(songs));
                    u.set_detail_all_songs(model.clone().into());
                    u.set_detail_songs(model.into());
                }
            });
            let covers = crate::load::fetch_covers(&urls).await;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(u) = ui_c.upgrade() {
                    if let Some(buf) = covers.get(&head_cover).and_then(|x| x.as_ref()) {
                        u.set_detail_cover(slint::Image::from_rgba8(buf.clone()));
                    }
                    let all = u.get_detail_all_songs();
                    for i in 0..all.row_count() {
                        let Some(s) = all.row_data(i) else { continue; };
                        let url = s.url.to_string();
                        if url.is_empty() { continue; }
                        if let Some(buf) = covers.get(&url).and_then(|x| x.as_ref()) {
                            let mut ns = s.clone();
                            ns.cover = slint::Image::from_rgba8(buf.clone());
                            all.set_row_data(i, ns);
                        }
                    }
                    // 若当前无搜索词，同步展示模型封面
                    if u.get_detail_search().trim().is_empty() {
                        let d = u.get_detail_songs();
                        for i in 0..d.row_count() {
                            let Some(s) = d.row_data(i) else { continue; };
                            if s.cover.size().width == 0 {
                                if let Some(all_s) = (0..u.get_detail_all_songs().row_count())
                                    .find_map(|j| u.get_detail_all_songs().row_data(j).filter(|a| a.id == s.id))
                                {
                                    let mut ns = s.clone();
                                    ns.cover = all_s.cover;
                                    d.set_row_data(i, ns);
                                }
                            }
                        }
                    }
                }
            });
        });
    });
}
