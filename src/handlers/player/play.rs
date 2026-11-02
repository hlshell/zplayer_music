//! 播放核心回调：换歌、播放全部（Hero/详情）、seek、播放/暂停、
//! 上一首/下一首、播放模式切换、播放进度轮询、定位正在播放、播放加载动画。

use super::{Ctx, Resolver};
use crate::core::lyrics;
use crate::core::queue;
use crate::persist::liked;
use crate::persist::recent;
use crate::Song;
use slint::{ComponentHandle, Model, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

pub(crate) fn setup_play(ctx: &Ctx, resolver: &Resolver) {
    // ---- 点击歌曲（换歌）：设当前曲目 → 记录 last-song（进度清零）→ 从头播放 ----
    let ui_weak = ctx.ui.clone();
    let toast_cb = ctx.toast.clone();
    let shared_ps = ctx.shared.clone();
    let resolver_cb = resolver.clone();
    ctx.app.clone_strong().on_play_song(move |id| {
        let ui = ui_weak.clone();
        let toast = toast_cb.clone();
        let mut found = None;
        if let Some(u) = ui.upgrade() {
            for i in 0..u.get_queue().row_count() {
                if let Some(s) = u.get_queue().row_data(i) {
                    if s.id == id {
                        found = Some((s.name.clone(), s.artist.clone(), s.album.clone(), s.duration_sec as u64, s.cover, s.url.clone()));
                        break;
                    }
                }
            }
        }
        // 队列未命中（详情页歌曲等）：从详情页列表补元数据并追加入列，保证行点击可播
        if found.is_none() {
            if let Some(u) = ui.upgrade() {
                let m = u.get_detail_songs();
                for i in 0..m.row_count() {
                    let Some(s) = m.row_data(i) else { continue; };
                    if s.id == id {
                        found = Some((s.name.clone(), s.artist.clone(), s.album.clone(), s.duration_sec as u64, s.cover.clone(), s.url.clone()));
                        let q = u.get_queue();
                        let mut list: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
                        list.push(s);
                        u.set_queue(Rc::new(VecModel::from(list)).into());
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            // 兜底 2：搜索结果列表（搜索页行点击播放，队列/详情页均无此歌时递增入列）
            if let Some(u) = ui.upgrade() {
                let m = u.get_search_results();
                for i in 0..m.row_count() {
                    let Some(s) = m.row_data(i) else { continue; };
                    if s.id == id {
                        found = Some((s.name.clone(), s.artist.clone(), s.album.clone(), s.duration_sec as u64, s.cover.clone(), s.url.clone()));
                        let q = u.get_queue();
                        let mut list: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
                        list.push(s);
                        u.set_queue(Rc::new(VecModel::from(list)).into());
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            // 兜底 3：Hero 每日推荐池（Hero 卡行内播放，队列/详情页/搜索结果均无此歌时）
            let pool = crate::load::HERO_POOL.lock().unwrap().clone();
            if let Some(d) = pool.iter().find(|d| d.id == id) {
                if let Some(u) = ui.upgrade() {
                    let cover_img = crate::load::cover_now(&d.url);
                    let song = Song {
                        id,
                        name: d.name.clone(),
                        artist: d.artist.clone(),
                        album: d.album.clone(),
                        cover: cover_img.clone(),
                        duration: d.duration.clone(),
                        duration_sec: d.duration_sec,
                        url: d.url.clone().into(),
                        liked: liked::is_liked(id as u32 as i64),
                    };
                    let q = u.get_queue();
                    let mut list: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
                    list.push(song);
                    u.set_queue(Rc::new(VecModel::from(list)).into());
                    found = Some((d.name.clone(), d.artist.clone(), d.album.clone(), d.duration_sec as u64, cover_img, d.url.clone().into()));
                }
            }
        }
        if found.is_none() {
            // 兜底 4：我的最爱 / 最近播放列表（行点击播放，队列/详情页/搜索/Hero 池均无此歌时）
            if let Some(u) = ui.upgrade() {
                for model in [u.get_liked_song_list(), u.get_recent_song_list()] {
                    let mut hit = None;
                    for i in 0..model.row_count() {
                        if let Some(s) = model.row_data(i) {
                            if s.id == id { hit = Some(s); break; }
                        }
                    }
                    if let Some(s) = hit {
                        found = Some((s.name.clone(), s.artist.clone(), s.album.clone(), s.duration_sec as u64, s.cover.clone(), s.url.clone()));
                        let q = u.get_queue();
                        let mut list: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
                        list.push(s);
                        u.set_queue(Rc::new(VecModel::from(list)).into());
                        break;
                    }
                }
            }
        }
        if found.is_none() {
            // 兜底 5：本地资源歌曲列表（行点击播放，上述列表均无此歌时）
            if let Some(u) = ui.upgrade() {
                let m = u.get_local_song_list();
                let mut hit = None;
                for i in 0..m.row_count() {
                    if let Some(s) = m.row_data(i) {
                        if s.id == id { hit = Some(s); break; }
                    }
                }
                if let Some(s) = hit {
                    found = Some((s.name.clone(), s.artist.clone(), s.album.clone(), s.duration_sec as u64, s.cover.clone(), s.url.clone()));
                    let q = u.get_queue();
                    let mut list: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
                    list.push(s);
                    u.set_queue(Rc::new(VecModel::from(list)).into());
                }
            }
        }
        let (name, artist, album, dur_sec, cover, cover_url) = found.unwrap_or_default();
        if name.is_empty() { return; }
        // 封面为空且有 URL（如搜索结果递增入列的行无缓存封面）：记录待后台补刷
        let cover_empty = cover.size().width == 0;
        let dur_str = format!("{:02}:{:02}", dur_sec / 60, dur_sec % 60);
        if let Some(u) = ui.upgrade() {
            let song = Song {
                id,
                name: name.clone(),
                artist: artist.clone(),
                album: album.clone(),
                cover,
                duration: dur_str.clone().into(),
                duration_sec: dur_sec as f32,
                url: cover_url.clone(),
                liked: liked::is_liked(id as u32 as i64),
            };
            u.set_current_song(song);
            liked::sync_liked(&u);
            crate::platform::audio::stop();
            crate::platform::audio::begin_switch();
            u.set_progress(0.0);
            u.set_playing(true);
            toast.show(&format!("正在播放：{}", name), "info");
            let uid = id as u32 as i64;
            queue::record_last_song(&shared_ps, &u, uid);
            // 记录最近播放（封面 URL 用 url 字段）
            recent::record(
                uid,
                name.to_string(),
                artist.to_string(),
                album.to_string(),
                cover_url.to_string(),
                dur_str.clone(),
                dur_sec as f32,
                0, // kind=歌曲
            );
            if u.get_current_nav() == 8 {
                super::super::fav::fill_recent(&u);
            }
            resolver_cb(uid, name.to_string(), artist.to_string(), dur_sec as f32, 0.0);
        }
        // 封面兜底补刷：统一走 cover_warm（dcache→磁盘→网络，同 URL 合并），
        // 成功后回刷 current_song 与队列对应行。
        // 修复搜索等递增入列路径底部播放栏/队列行封面空白。
        // 本地歌曲 url 为文件路径而非封面 URL，跳过补刷。
        let is_local = !cover_url.is_empty() && !cover_url.starts_with("http");
        if cover_empty && !cover_url.is_empty() && !is_local {
            let ui_c = ui.clone();
            let url_c = cover_url.to_string();
            let url_ok = url_c.clone();
            crate::load::cover_warm(&url_c, crate::persist::cache::CoverType::Song, move |img| {
                let Some(u) = ui_c.upgrade() else { return; };
                let mut cur = u.get_current_song();
                if cur.id == id && cur.url.as_str() == url_ok.as_str() {
                    cur.cover = img.clone();
                    u.set_current_song(cur);
                }
                let q = u.get_queue();
                for i in 0..q.row_count() {
                    if let Some(mut row) = q.row_data(i) {
                        if row.id == id {
                            row.cover = img.clone();
                            q.set_row_data(i, row);
                        }
                    }
                }
            });
        }
    });

    // ---- 首页 Hero"播放全部"：池内不在队列的歌曲追加进播放列表，播放第一首 ----
    let ui_weak = ctx.ui.clone();
    let toast_hero = ctx.toast.clone();
    let rt_hero = ctx.rt.clone();
    ctx.app.clone_strong().on_hero_play_all(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let pool = crate::load::HERO_POOL.lock().unwrap().clone();
        if pool.is_empty() {
            toast_hero.show("暂无可加入的歌曲", "info");
            return;
        }
        let q = u.get_queue();
        let mut songs: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
        let mut have: std::collections::HashSet<i64> = songs.iter().map(|s| s.id as u32 as i64).collect();
        let mut added = 0usize;
        for d in pool.iter() {
            let xid = d.id as u32 as i64;
            // insert 含去重语义：队列已有 + 池内重复都跳过
            if !have.insert(xid) { continue; }
            songs.push(Song {
                id: d.id,
                name: d.name.clone(),
                artist: d.artist.clone(),
                album: d.album.clone(),
                cover: crate::load::cover_now(&d.url),
                duration: d.duration.clone(),
                duration_sec: d.duration_sec,
                url: d.url.clone().into(),
                liked: liked::is_liked(xid),
            });
            added += 1;
        }
        u.set_queue(Rc::new(VecModel::from(songs)).into());
        if let Some(first) = pool.first() {
            u.invoke_play_song(first.id);
        }
        if added > 0 {
            toast_hero.show(&format!("已加入 {} 首歌曲到播放列表", added), "info");
        } else {
            toast_hero.show("歌曲已在播放列表", "info");
        }
        // 后台补齐新增歌曲封面（磁盘缓存优先），完成后逐行刷新队列
        let ui_p = ui_weak.clone();
        rt_hero.spawn(async move {
            let urls: Vec<(String, crate::persist::cache::CoverType)> =
                pool.iter().map(|d| (d.url.clone(), crate::persist::cache::CoverType::Song)).collect();
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
    });

    // ---- 详情页"播放全部"：详情列表不在队列的追加进播放列表，播放详情第一首 ----
    let ui_weak = ctx.ui.clone();
    let toast_dpa = ctx.toast.clone();
    ctx.app.clone_strong().on_detail_play_all(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let m = u.get_detail_songs();
        if m.row_count() == 0 {
            toast_dpa.show("暂无可播放的歌曲", "info");
            return;
        }
        let q = u.get_queue();
        let mut songs: Vec<Song> = (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
        let have: std::collections::HashSet<i64> = songs.iter().map(|s| s.id as u32 as i64).collect();
        let mut added = 0usize;
        let mut first_id = 0i32;
        for i in 0..m.row_count() {
            let Some(s) = m.row_data(i) else { continue; };
            if first_id == 0 { first_id = s.id; }
            let xid = s.id as u32 as i64;
            if have.contains(&xid) { continue; }
            songs.push(s);
            added += 1;
        }
        u.set_queue(Rc::new(VecModel::from(songs)).into());
        if first_id != 0 { u.invoke_play_song(first_id); }
        if added > 0 {
            toast_dpa.show(&format!("已加入 {} 首歌曲到播放列表", added), "info");
        } else {
            toast_dpa.show("歌曲已在播放列表", "info");
        }
    });

    // ---- 进度条点击跳转：以重播方式实现真实 seek ----
    let ui_weak = ctx.ui.clone();
    let resolver_cb = resolver.clone();
    ctx.app.clone_strong().on_seek(move |f| {
        let Some(u) = ui_weak.clone().upgrade() else { return; };
        let f = f.clamp(0.0, 1.0);
        let cur = u.get_current_song();
        let dur = cur.duration_sec;
        if dur <= 0.0 { return; }
        let target = if f * dur < 1.0 { 0.0 } else { f * dur };
        let uid = cur.id as u32 as i64;
        crate::platform::audio::begin_switch();
        u.set_progress(target / dur);
        u.set_playing(true);
        resolver_cb(uid, cur.name.to_string(), cur.artist.to_string(), dur, target);
    });

    // ---- 播放/暂停：无活动播放且是上次歌曲时续播，否则正常切换 ----
    let ui_weak = ctx.ui.clone();
    let toast_cb = ctx.toast.clone();
    let shared_pt = ctx.shared.clone();
    let resolver_cb = resolver.clone();
    ctx.app.clone_strong().on_play_toggle(move || {
        let Some(u) = ui_weak.clone().upgrade() else { return; };
        let (p, playing) = crate::platform::audio::status();
        if p > 0.0 || playing {
            crate::platform::audio::toggle();
            return;
        }
        let cur = u.get_current_song();
        let cur_uid = cur.id as u32 as i64;
        let (last_id, last_progress, last_name, last_artist, last_dur) = {
            let g = shared_pt.borrow();
            (
                g.last_song.id,
                g.last_song.progress_sec,
                g.last_song.name.clone(),
                g.last_song.artist.clone(),
                g.last_song.duration_sec,
            )
        };
        if last_id != 0
            && cur_uid == last_id
            && last_progress > 2.0
            && last_progress < last_dur - 3.0
            && last_dur > 0.0
        {
            crate::platform::audio::begin_switch();
            u.set_playing(true);
            toast_cb.show(&format!("续播：{}", last_name), "info");
            resolver_cb(last_id, last_name, last_artist, last_dur, last_progress);
        } else if cur.duration_sec > 0.0 {
            crate::platform::audio::begin_switch();
            u.set_playing(true);
            resolver_cb(cur_uid, cur.name.to_string(), cur.artist.to_string(), cur.duration_sec, 0.0);
        } else {
            crate::platform::audio::toggle();
        }
    });

    // ---- 播放进度轮询：由真实音频驱动 progress / playing；播完自动按播放模式下一首 ----
    let ui_weak = ctx.ui.clone();
    let shared_ptick = ctx.shared.clone();
    let resolver_cb = resolver.clone();
    let auto_shared = ctx.shared.clone();
    let rt_io = ctx.rt.clone();
    // 缓存占用统计在飞标志：避免后台扫描重叠（全目录递归，文件多时单次几十 ms）
    let usage_busy_t = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let auto_was_playing = Rc::new(Cell::new(false));
    let auto_wp = auto_was_playing.clone();
    let retry_cnt = Rc::new(Cell::new(0u32));
    let retry_cb = retry_cnt.clone();
    let prog_timer = Rc::new(RefCell::new(slint::Timer::default()));
    let tick = Rc::new(Cell::new(0u32));
    let tick_cb = tick.clone();
    prog_timer.borrow().start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let (p, playing) = crate::platform::audio::status();
        let sec = p * u.get_current_song().duration_sec;
        if crate::platform::audio::switching() {
            // 切歌/seek 过渡期：保持 UI 现状，避免闪回 0/播放态
        } else if crate::platform::audio::active() {
            auto_wp.set(playing);
            if playing { retry_cb.set(0); } // 正常出声即复位失败重试计数
            u.set_progress(p);
            u.set_playing(playing);
            // 当前行定位：歌词按时间升序（parse_lrc 已排序），用二分取代线性扫描；
            // 无任何行时间 <= 当前进度时回落第 0 行（与旧线性实现一致）
            let idx = {
                let lyr = lyrics::LYRICS.lock().unwrap();
                lyr.partition_point(|(t, _)| *t <= sec).saturating_sub(1)
            };
            u.set_current_lyric(idx as i32);
            // 逐词歌词：推送当前播放毫秒（250ms 周期），驱动行内进度填充
            u.set_current_time_ms((sec * 1000.0) as i32);
        } else {
            // 同值 set 会反复脏化属性触发重绘，空闲时先判再写
            if u.get_progress() != 0.0 { u.set_progress(0.0); }
            if u.get_playing() { u.set_playing(false); }
            // 仅当进度接近结尾（剩余 <3s，且时长已知>0）才算"播完"→ 按播放模式自动下一首；
            // 否则视为播放中断（缓冲/解码短暂闪断，或时长未知），不切歌，
            // 避免误切仍可正常播放的歌曲。时长未知(dur<=0)时保守不自动切。
            if auto_wp.get() {
                auto_wp.set(false);
                if crate::platform::audio::finished() {
                    // 真播完（sink 播空）→ 按播放模式自动切下一首
                    if let Some(idx) = queue::pick_queue_target(&u, u.get_play_mode(), 1) {
                        queue::play_queue_index(&u, idx, &auto_shared, &*resolver_cb);
                    }
                } else if retry_cb.get() < 2 {
                    // 中途失败（解码/流断，未播完）→ 换源重播当前歌；连续 2 次仍失败才切下一首
                    retry_cb.set(retry_cb.get() + 1);
                    let song = u.get_current_song();
                    let rid = song.id as u32 as i64;
                    crate::platform::audio::begin_switch();
                    u.set_playing(true);
                    resolver_cb(
                        rid,
                        song.name.to_string(),
                        song.artist.to_string(),
                        song.duration_sec,
                        0.0,
                    );
                } else {
                    // 连续重播失败：真正无法播放 → 切下一首（由 resolver 失败分支 Toast/计数）
                    retry_cb.set(0);
                    if let Some(idx) = queue::pick_queue_target(&u, u.get_play_mode(), 1) {
                        queue::play_queue_index(&u, idx, &auto_shared, &*resolver_cb);
                    }
                }
            } else {
                auto_wp.set(false);
            }
        }
        // 每 5s（20 tick）节流保存播放进度（防强杀丢进度的兜底落盘）
        let n = tick_cb.get() + 1;
        tick_cb.set(n);
        if n % 20 == 0 {
            if p > 0.0 || playing {
                // UI 线程只做内存更新 + 序列化；落盘交给阻塞线程池，
                // 避免播放期间每 5s 一次"写临时文件 + rename"落进 UI 线程。
                let text = {
                    let mut g = shared_ptick.borrow_mut();
                    g.last_song.progress_sec = sec;
                    g.last_song.duration_sec = u.get_current_song().duration_sec;
                    toml::to_string_pretty(&*g).ok()
                };
                if let Some(s) = text {
                    rt_io.spawn_blocking(move || {
                        let _ = crate::platform::storage::write_text("config.toml", &s);
                    });
                }
            }
            // 缓存占用 = 全缓存目录递归遍历，文件多时单次可达几十 ms。
            // 放到阻塞线程池执行，完成后回 UI 线程回填；在飞标志防止扫描重叠。
            if u.get_current_nav() == 99 && n % 40 == 0
                && !usage_busy_t.swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                let ui_bg = ui_weak.clone();
                let busy_bg = usage_busy_t.clone();
                rt_io.spawn_blocking(move || {
                    let mb = (crate::persist::cache::total_usage() / (1024 * 1024)) as i32;
                    busy_bg.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(u2) = ui_bg.upgrade() {
                            u2.set_cache_usage_mb(mb);
                        }
                    });
                });
            }
        }
    });
    // Timer 需持续存活（drop 即停止）→ leak 持有
    std::mem::forget(prog_timer);

    // ---- 上一首/下一首 / 播放模式 ----
    let ui_weak = ctx.ui.clone();
    let resolver_cb = resolver.clone();
    let shared_pv = ctx.shared.clone();
    ctx.app.clone_strong().on_prev_song(move || {
        if let Some(u) = ui_weak.upgrade() {
            if let Some(idx) = queue::pick_queue_target(&u, u.get_play_mode(), -1) {
                queue::play_queue_index(&u, idx, &shared_pv, &*resolver_cb);
            }
        }
    });
    let ui_weak = ctx.ui.clone();
    let resolver_cb = resolver.clone();
    let shared_nx = ctx.shared.clone();
    ctx.app.clone_strong().on_next_song(move || {
        if let Some(u) = ui_weak.upgrade() {
            if let Some(idx) = queue::pick_queue_target(&u, u.get_play_mode(), 1) {
                queue::play_queue_index(&u, idx, &shared_nx, &*resolver_cb);
            }
        }
    });
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_play_mode_changed(move || {
        if let Some(u) = ui_weak.upgrade() {
            u.set_play_mode((u.get_play_mode() + 1) % 3);
        }
    });

    // ---- 打开播放列表抽屉时定位正在播放歌曲：算当前曲在队列中的行索引写入 ----
    // 先置 -1 再延迟写索引：changed 回调可能合并同轮赋值（同索引重开抽屉净变化为 0 不触发），
    // 且抽屉刚 visible 时布局尚未完成（content-height 可能是旧值），延迟一帧确保滚动钳制正确
    let ui_weak = ctx.ui.clone();
    ctx.app.clone_strong().on_locate_playing(move || {
        let Some(u) = ui_weak.upgrade() else { return; };
        let cur = u.get_current_song().id;
        let q = u.get_queue();
        let mut idx = -1i32;
        for i in 0..q.row_count() {
            if let Some(s) = q.row_data(i) {
                if s.id == cur {
                    idx = i as i32;
                    break;
                }
            }
        }
        u.set_playlist_scroll_index(-1);
        if idx >= 0 {
            let uw = ui_weak.clone();
            slint::Timer::single_shot(Duration::from_millis(80), move || {
                if let Some(u) = uw.upgrade() {
                    u.set_playlist_scroll_index(idx);
                }
            });
        }
    });

    // ---- 播放加载动画驱动：见 kick_loading_anim（按需启停，闲置零唤醒）----
}

thread_local! {
    /// 加载动画定时器句柄：仅"正在取音频"期间存在，加载结束即停并置空。
    static ANIM_TIMER: std::cell::RefCell<Option<Rc<slint::Timer>>> = const { std::cell::RefCell::new(None) };
}

/// 启动播放加载动画（封面 loader 旋转 + 进度条脉冲，40ms/帧）。
/// 幂等：已在运行时直接返回；定时器检测到 playing_loading=false 会自停并清空句柄，
/// 避免此前"40ms 定时器永久空转（25 次/秒唤醒）"。
pub(crate) fn kick_loading_anim(ui: &slint::Weak<crate::AppWindow>) {
    ANIM_TIMER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let timer = Rc::new(slint::Timer::default());
        let timer_cb = timer.clone();
        let anim_ui = ui.clone();
        let pulse_dir = Rc::new(Cell::new(1.0f32));
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(40), move || {
            let Some(u) = anim_ui.upgrade() else {
                timer_cb.stop();
                ANIM_TIMER.with(|s| *s.borrow_mut() = None);
                return;
            };
            if !u.get_playing_loading() {
                // 加载结束 → 自停并释放句柄，回到零唤醒
                timer_cb.stop();
                ANIM_TIMER.with(|s| *s.borrow_mut() = None);
                return;
            }
            u.set_playing_loader_angle(u.get_playing_loader_angle() + 8.0);
            let p = u.get_loading_pulse() + 0.08 * pulse_dir.get();
            if p >= 1.0 { pulse_dir.set(-1.0); }
            else if p <= 0.0 { pulse_dir.set(1.0); }
            u.set_loading_pulse(p.clamp(0.0, 1.0));
        });
        *slot.borrow_mut() = Some(timer);
    });
}
