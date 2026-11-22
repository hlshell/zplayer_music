//! 移动端回调层（与桌面 `src/handlers` 独立）：首页点歌、播放控制、进度轮询。
//!
//! 队列保存在 `MobileCtx`，与 AppWindow 的 `queue` 模型保持同步；
//! 音频解码在 tokio 运行时后台执行，播放门面走 `crate::platform::audio`。
//! 所有回调均在 UI 线程触发（Slint 回调语义），因此可直接读写 UI 属性。

use crate::network::netease::client::NeteaseClient;
use crate::platform::audio as player;
use slint::{ComponentHandle, Model};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// 移动端 UI 回调共享上下文。
pub struct MobileCtx {
    pub ui: slint::Weak<crate::AppWindow>,
    pub rt: tokio::runtime::Handle,
    pub client: Arc<NeteaseClient>,
    /// 播放队列（与 UI queue 模型同步；首页数据到达后首次点歌/切歌时刷新）
    pub queue: RefCell<Vec<crate::Song>>,
    /// 当前曲目在队列中的索引
    pub cur: Cell<usize>,
    /// 播放模式 0=列表循环 1=单曲循环 2=顺序循环
    pub mode: Cell<i32>,
    /// 搜索请求序号：每次发起搜索自增。响应回到 UI 线程后先比对序号，过期响应直接丢弃。
    /// 用 `Arc<AtomicU64>` 而非 `Cell`：回投到事件循环的闭包必须 Send，不能捕获 Rc/Cell。
    pub search_seq: Arc<AtomicU64>,
    /// 最近一次有效搜索关键词（切换类型时复用）
    pub search_kw: RefCell<String>,
}

/// 读取 UI 侧 queue 模型为 Vec（仅 UI 线程使用）。
fn read_ui_queue(ui: &crate::AppWindow) -> Vec<crate::Song> {
    let q = ui.get_queue();
    let mut out = Vec::with_capacity(q.row_count());
    for i in 0..q.row_count() {
        if let Some(s) = q.row_data(i) {
            out.push(s);
        }
    }
    out
}

/// 取当前队列快照：UI 队列非空时以其为准并回写 ctx；否则沿用 ctx 内队列
/// （启动早期首页数据尚未到达，UI 队列仍为空）。
fn queue_snapshot(ctx: &MobileCtx, ui: &crate::AppWindow) -> Vec<crate::Song> {
    let ui_q = read_ui_queue(ui);
    if ui_q.is_empty() {
        return ctx.queue.borrow().clone();
    }
    *ctx.queue.borrow_mut() = ui_q.clone();
    ui_q
}

/// 从首页数据池解析歌曲元数据（Hero 精选池 → 新歌速递池）。
fn resolve_from_pools(id: i32) -> Option<crate::Song> {
    let hero = {
        let pool = crate::load::HERO_POOL.lock().ok()?;
        pool.iter().find(|s| s.id == id).cloned()
    };
    if let Some(b) = hero {
        return Some(song_from_base(&b));
    }
    let ns = {
        let pool = crate::load::NEW_SONG_POOL.lock().ok()?;
        pool.iter().find(|s| s.id == id).cloned()
    };
    ns.as_ref().map(song_from_base)
}

/// 在 UI 模型里按 id 找歌（搜索结果/详情列表等非队列来源）。
fn find_in_model(m: &slint::ModelRc<crate::Song>, id: i32) -> Option<crate::Song> {
    for i in 0..m.row_count() {
        if let Some(s) = m.row_data(i) {
            if s.id == id {
                return Some(s);
            }
        }
    }
    None
}

fn song_from_base(b: &crate::load::SongBase) -> crate::Song {
    let cover_url = b.url.clone();
    crate::Song {
        id: b.id,
        name: b.name.clone(),
        artist: b.artist.clone(),
        album: b.album.clone(),
        cover: crate::load::cover_now(&cover_url),
        duration: b.duration.clone(),
        duration_sec: b.duration_sec,
        url: cover_url.into(),
        liked: crate::persist::liked::is_liked(b.id as u32 as i64),
    }
}

/// 搜索结果中间态（仅含 Send 数据；slint::Image 只能在 UI 线程构造）。
enum SearchHits {
    Songs(Vec<crate::load::SongBase>),
    Playlists(Vec<PlaylistHit>),
    Albums(Vec<AlbumHit>),
}

struct PlaylistHit {
    id: i32,
    name: slint::SharedString,
    plays: slint::SharedString,
    url: String,
}

struct AlbumHit {
    id: i32,
    name: slint::SharedString,
    artist: slint::SharedString,
    year: slint::SharedString,
    url: String,
}

/// publishTime（毫秒时间戳）→ 年份字符串；缺失/非法留空。
fn ms_to_year(ms: i64) -> slint::SharedString {
    if ms <= 0 {
        return slint::SharedString::default();
    }
    ((1970.0 + (ms / 1000 / 86400) as f64 / 365.25) as i64).to_string().into()
}

/// 后台线程解析响应：只产出 Send 结构，不碰 slint::Image。
fn parse_hits(v: &serde_json::Value, ty: i32) -> SearchHits {
    match ty {
        0 => {
            let arr = v
                .pointer("/result/songs")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Songs(crate::load::parse_tracks(&arr))
        }
        1 => {
            let arr = v
                .pointer("/result/playlists")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Playlists(
                arr.iter()
                    .map(|p| PlaylistHit {
                        id: p.get("id").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                        name: p.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                        plays: crate::core::song_util::fmt_plays_cat(
                            p.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0),
                        )
                        .into(),
                        url: p
                            .get("coverImgUrl")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect(),
            )
        }
        _ => {
            let arr = v
                .pointer("/result/albums")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            SearchHits::Albums(
                arr.iter()
                    .map(|a| AlbumHit {
                        id: a.get("id").and_then(|x| x.as_i64()).unwrap_or(0) as i32,
                        name: a.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                        artist: a
                            .get("artist")
                            .and_then(|x| x.get("name"))
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .into(),
                        year: a
                            .get("publishTime")
                            .and_then(|x| x.as_i64())
                            .map(ms_to_year)
                            .unwrap_or_default(),
                        url: a
                            .get("picUrl")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect(),
            )
        }
    }
}

/// UI 线程落库：封面/渐变在此构造（slint::Image 非 Send）。
fn apply_hits(ui: &crate::AppWindow, hits: SearchHits) {
    match hits {
        SearchHits::Songs(items) => {
            let rows: Vec<crate::Song> = items
                .iter()
                .map(|b| crate::Song {
                    id: b.id,
                    name: b.name.clone(),
                    artist: b.artist.clone(),
                    album: b.album.clone(),
                    cover: crate::load::cover_now(&b.url),
                    duration: b.duration.clone(),
                    duration_sec: b.duration_sec,
                    url: b.url.clone().into(),
                    liked: crate::persist::liked::is_liked(b.id as u32 as i64),
                })
                .collect();
            ui.set_search_songs(crate::load::model(rows));
        }
        SearchHits::Playlists(items) => {
            let rows: Vec<crate::Playlist> = items
                .iter()
                .enumerate()
                .map(|(i, p)| crate::Playlist {
                    id: p.id,
                    name: p.name.clone(),
                    plays: p.plays.clone(),
                    cover: crate::load::cover_now(&p.url),
                    from: crate::load::from(i),
                    to: crate::load::to(i),
                })
                .collect();
            ui.set_search_playlists(crate::load::model(rows));
        }
        SearchHits::Albums(items) => {
            let rows: Vec<crate::Album> = items
                .iter()
                .enumerate()
                .map(|(i, a)| crate::Album {
                    id: a.id,
                    name: a.name.clone(),
                    artist: a.artist.clone(),
                    year: a.year.clone(),
                    cover: crate::load::cover_now(&a.url),
                    from: crate::load::from(i),
                    to: crate::load::to(i),
                })
                .collect();
            ui.set_search_albums(crate::load::model(rows));
        }
    }
}

/// 发起一次搜索：序号自增 → 置加载态 → 后台请求 → UI 线程按序号防串回填。
fn start_search(ctx: &Rc<MobileCtx>, keyword: &str, ty: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let kw = keyword.trim().to_string();

    // 空关键词：清空结果并作废在途请求（序号自增让旧响应失效）
    if kw.is_empty() {
        ctx.search_seq.fetch_add(1, Ordering::SeqCst);
        ctx.search_kw.borrow_mut().clear();
        ui.set_search_songs(crate::load::model(Vec::new()));
        ui.set_search_playlists(crate::load::model(Vec::new()));
        ui.set_search_albums(crate::load::model(Vec::new()));
        ui.set_search_loading(false);
        ui.set_search_error("".into());
        ui.set_search_keyword("".into());
        return;
    }

    let seq = ctx.search_seq.fetch_add(1, Ordering::SeqCst) + 1;
    *ctx.search_kw.borrow_mut() = kw.clone();

    ui.set_search_type(ty);
    ui.set_search_keyword(kw.as_str().into());
    ui.set_search_loading(true);
    ui.set_search_error("".into());

    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    let seq_now = ctx.search_seq.clone();
    ctx.rt.spawn(async move {
        // 本地类型 → 网易云搜索类型：0 单曲=1 / 1 歌单=1000 / 2 专辑=10
        let api_ty = match ty {
            0 => 1,
            1 => 1000,
            _ => 10,
        };
        let result = crate::network::service::search(&client, &kw, api_ty, 30, 0).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui_weak.upgrade() else { return; };
            // 序号防串：期间已有新搜索发起（或已清空）则丢弃本次响应
            if seq_now.load(Ordering::SeqCst) != seq {
                return;
            }
            match result {
                Ok(v) => {
                    apply_hits(&u, parse_hits(&v, ty));
                    u.set_search_error("".into());
                }
                Err(e) => {
                    log::warn!("[mobile] 搜索失败: keyword={kw} err={e}");
                    u.set_search_error("搜索失败，请重试".into());
                }
            }
            u.set_search_loading(false);
        });
    });
}

/// 播放指定歌曲：解析元数据 → 同步队列 → 更新 UI → 后台取音频并播放。
fn play_song(ctx: &Rc<MobileCtx>, id: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let q = queue_snapshot(ctx, &ui);

    let mut idx = q.iter().position(|s| s.id == id);
    let mut song = if let Some(i) = idx {
        q[i].clone()
    } else {
        // 队列未命中：首页池 → 搜索结果列表（搜索页行点击播放）
        match resolve_from_pools(id).or_else(|| find_in_model(&ui.get_search_songs(), id)) {
            Some(s) => s,
            None => {
                eprintln!("[mobile] 未找到歌曲元数据: id={id}");
                return;
            }
        }
    };

    // 封面补图：队列行可能尚无封面（空图）时用 UI 线程内存/磁盘缓存补齐
    if song.cover.size().width == 0 {
        let img = crate::load::cover_now(&song.url);
        if img.size().width > 0 {
            song.cover = img;
        }
    }

    // 不在队列则追加，并同步回 UI 模型
    if idx.is_none() {
        ctx.queue.borrow_mut().push(song.clone());
        idx = Some(ctx.queue.borrow().len().saturating_sub(1));
        ui.set_queue(crate::load::model(ctx.queue.borrow().clone()));
    }
    ctx.cur.set(idx.unwrap_or(0));

    ui.set_current_song(song.clone());
    ui.set_progress(0.0);
    ui.set_playing(true);

    // 即刻停旧流并进入切换过渡态，点击反馈不闪 UI
    player::stop();
    player::begin_switch();

    // Slint int 是 i32：大 ID 存为负数，须经 u32 往返还原真实网易云 ID
    let uid = song.id as u32 as i64;
    let dur_sec = song.duration_sec.max(0.0);
    let name = song.name.to_string();
    let artist = song.artist.to_string();
    let client = ctx.client.clone();
    let ui_weak = ctx.ui.clone();
    ctx.rt.spawn(async move {
        let bytes = crate::fetch::fetch_song_bytes(&client, uid, &name, &artist, dur_sec as u64).await;
        match bytes {
            Some(b) => {
                player::play_at(b, dur_sec, 0.0);
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(true);
                    }
                });
            }
            None => {
                player::end_switch();
                let ui_set = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        u.set_playing(false);
                    }
                });
                eprintln!("[mobile] 音频获取失败: id={uid} name={name}");
            }
        }
    });
}

/// 按播放模式切换上一首/下一首：dir=1 下一首，dir=-1 上一首。
fn step_song(ctx: &Rc<MobileCtx>, dir: i32) {
    let Some(ui) = ctx.ui.upgrade() else { return; };
    let q = queue_snapshot(ctx, &ui);
    let n = q.len();
    if n == 0 {
        ui.set_playing(false);
        return;
    }
    let cur_id = ui.get_current_song().id;
    let cur = q
        .iter()
        .position(|s| s.id == cur_id)
        .unwrap_or_else(|| ctx.cur.get().min(n - 1));
    let target = if ctx.mode.get() == 1 {
        // 单曲循环：上一首/下一首均重播当前
        cur
    } else {
        // 列表循环 / 顺序循环：按序移动并回绕
        (((cur as i32 + dir) % n as i32) + n as i32) as usize % n
    };
    ctx.cur.set(target);
    play_song(ctx, q[target].id);
}

/// 安装移动端全部回调与进度轮询。
pub fn setup(ui: &crate::AppWindow, rt: tokio::runtime::Handle, client: Arc<NeteaseClient>) {
    ui.set_play_mode(0);
    ui.set_playing(false);

    let ctx = Rc::new(MobileCtx {
        ui: ui.as_weak(),
        rt,
        client,
        queue: RefCell::new(read_ui_queue(ui)),
        cur: Cell::new(0),
        mode: Cell::new(0),
        search_seq: Arc::new(AtomicU64::new(0)),
        search_kw: RefCell::new(String::new()),
    });

    // ---- 首页点歌 ----
    let c = ctx.clone();
    ui.on_song_played(move |id| play_song(&c, id));

    // 歌单打开：详情页尚未接入，暂仅记录
    ui.on_playlist_opened(move |id| {
        log::info!("[mobile] 打开歌单: id={id}");
    });

    // ---- 搜索 ----
    let c = ctx.clone();
    ui.on_search_submitted(move |keyword, ty| start_search(&c, keyword.as_str(), ty));

    let c = ctx.clone();
    ui.on_search_type_changed(move |ty| {
        if let Some(u) = c.ui.upgrade() {
            u.set_search_type(ty);
        }
        // 保留旧结果直到新结果到达（期间 loading=true）
        let kw = c.search_kw.borrow().clone();
        if !kw.is_empty() {
            start_search(&c, &kw, ty);
        }
    });

    // 专辑打开：详情页在后续任务接入，暂仅记录
    ui.on_album_opened(move |id| {
        log::info!("[mobile] 打开专辑: id={id}");
    });

    // ---- 播放控制 ----
    let c = ctx.clone();
    ui.on_player_toggle(move || {
        if player::active() {
            player::toggle();
            if let Some(u) = c.ui.upgrade() {
                let (_, playing) = player::status();
                u.set_playing(playing);
            }
            return;
        }
        let Some(u) = c.ui.upgrade() else { return; };
        let cur_id = u.get_current_song().id;
        // 尚无当前曲目时：取队列首曲（首页数据已到达时队列非空）
        let id = if cur_id != 0 {
            cur_id
        } else {
            let q = queue_snapshot(&c, &u);
            match q.first() {
                Some(s) => s.id,
                None => return,
            }
        };
        drop(u);
        play_song(&c, id);
    });

    let c = ctx.clone();
    ui.on_player_next(move || step_song(&c, 1));
    let c = ctx.clone();
    ui.on_player_prev(move || step_song(&c, -1));

    let c = ctx.clone();
    ui.on_player_mode(move || {
        let next = (c.mode.get() + 1) % 3;
        c.mode.set(next);
        if let Some(u) = c.ui.upgrade() {
            u.set_play_mode(next);
        }
    });

    // ---- 250ms 进度轮询 ----
    let c = ctx.clone();
    let was_playing = Rc::new(Cell::new(false));
    let timer = Rc::new(slint::Timer::default());
    let was = was_playing.clone();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(250),
        move || {
            let Some(ui) = c.ui.upgrade() else { return; };
            if player::switching() {
                return;
            }
            if player::active() {
                let (p, playing) = player::status();
                ui.set_progress(p);
                ui.set_playing(playing);
                was.set(playing);
            } else if was.get() && player::finished() {
                // 正常播完：自动切下一首（失败/切换中不触发）
                was.set(false);
                step_song(&c, 1);
            }
        },
    );
    // Timer 需要与事件循环同生命周期；主动持有到进程结束
    std::mem::forget(timer);
}
