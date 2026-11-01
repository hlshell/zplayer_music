//! web 播放业务：gdmusic 取流 + HTMLAudio 播放 + 队列 / 进度轮询 / 歌词 / 收藏。
//!
//! 归属回调（appwindow.slint root，唯一接线方，勿与其他模块重复）：
//!   play-song(int) / play-toggle / prev-song / next-song / play-mode-changed / seek(float) /
//!   volume-changed(float) / like-toggle / like-toggle-id(int) / clear-queue / remove-song(int) /
//!   request-lyrics(int) / hero-play-all / detail-play-all / locate-playing / mini-queue-toggle
//!
//! 数据流：
//!   UI id(i32) --gd::lookup_full--> (source, gd_id, name, artist, album, duration_sec)
//!              --gd::song_url(gd_id, source, DEFAULT_BR)--> 音频 URL
//!              --platform::audio::play_url(url, dur)--> HTMLAudioElement 出声
//!   audio::onended 写 STATE.finished → 250ms Timer 轮询 finished() 按 play-mode 自动切歌。
//!
//! 内部状态（thread_local）：
//!   LRC            当前歌曲的解析歌词 (ui_id, [(sec, text)])，供 Timer 推进高亮行
//!   RND            伪随机数（播放模式=2 随机选曲用）
//!   FINISHED_SEEN  finished 事件已消费标记（onended 后只自动切一次歌，失败不刷屏重试）

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, VecModel};

use crate::web::gd;
use crate::web::show_toast;

/// 当前歌曲歌词：(ui_id, [(秒, 文本)])。切换歌曲时整体替换。
thread_local! {
    static LRC: RefCell<(i32, Vec<(f32, String)>)> = const { RefCell::new((0, Vec::new())) };
    /// 线性同余伪随机（随机播放选曲）。
    static RND: RefCell<u32> = const { RefCell::new(1) };
    /// onended 已消费：一轮 finished 只触发一次自动切歌。
    static FINISHED_SEEN: RefCell<bool> = const { RefCell::new(false) };
}

// ---- 队列 / 当前曲 / 播放模式持久化（localStorage，键前缀 zplayer/）----
// 刷新后 UI id 全部重分配：持久化与恢复以 gd_id 字符串为准，启动时 register_full 重新登记。
// 收藏/最近播放持久化在 gd.rs（键 "liked"/"recent"），本模块勿重复。
const QUEUE_KEY: &str = "queue";
const QUEUE_CURRENT_KEY: &str = "queue-current";
const PLAY_MODE_KEY: &str = "play-mode";

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistQueueItem {
    source: String,
    gd_id: String,
    name: String,
    artist: String,
    album: String,
    duration_sec: f32,
    cover_url: String,
}

/// 队列中某 UI id 行的封面 URL（row.url）；无则空串。
fn queue_row_url(u: &crate::AppWindow, id: i32) -> String {
    let q = u.get_queue();
    for i in 0..q.row_count() {
        if let Some(s) = q.row_data(i) {
            if s.id == id {
                return s.url.to_string();
            }
        }
    }
    String::new()
}

/// 队列行异步补封面：url 非空即 spawn_cover_url，成功后 set_row_data（每模型 ≤30 行；失败静默保持渐变）。
fn spawn_queue_covers(u: &crate::AppWindow) {
    let q = u.get_queue();
    let n = q.row_count().min(30);
    for i in 0..n {
        let Some(row) = q.row_data(i) else { continue };
        let url = row.url.to_string();
        if url.is_empty() {
            continue;
        }
        let id = row.id;
        let weak = u.as_weak();
        gd::spawn_cover_url(url, move |img| {
            if let (Some(u), Some(img)) = (weak.upgrade(), img) {
                let q = u.get_queue();
                if let Some(row) = q.row_data(i) {
                    if row.id == id {
                        let mut ns = row.clone();
                        ns.cover = img;
                        q.set_row_data(i, ns);
                    }
                }
            }
        });
    }
}

/// 当前曲异步补封面：取队列中同 id 行 url，成功后写 current-song.cover（失败静默）。
fn spawn_current_cover(u: &crate::AppWindow) {
    let id = u.get_current_song().id;
    if id == 0 {
        return;
    }
    let url = queue_row_url(u, id);
    if url.is_empty() {
        return;
    }
    let weak = u.as_weak();
    gd::spawn_cover_url(url, move |img| {
        if let (Some(u), Some(img)) = (weak.upgrade(), img) {
            let mut cur = u.get_current_song();
            if cur.id == id {
                cur.cover = img;
                u.set_current_song(cur);
            }
        }
    });
}

/// 落盘队列 + 当前曲 gd_id + 播放模式（每次变更即写，localStorage 同步 API）。
fn persist_write(u: &crate::AppWindow) {
    let q = u.get_queue();
    let items: Vec<PersistQueueItem> = (0..q.row_count())
        .filter_map(|i| q.row_data(i))
        .filter_map(|row| {
            let (source, gd_id, _n, _a, _al, _dur) = gd::lookup_full(row.id)?;
            Some(PersistQueueItem {
                source,
                gd_id,
                name: row.name.to_string(),
                artist: row.artist.to_string(),
                album: row.album.to_string(),
                duration_sec: row.duration_sec,
                cover_url: row.url.to_string(),
            })
        })
        .collect();
    let _ = crate::platform::storage::write_text(
        QUEUE_KEY,
        &serde_json::to_string(&items).unwrap_or_default(),
    );
    let cur_gd = gd::lookup_full(u.get_current_song().id)
        .map(|(_, g, _, _, _, _)| g)
        .unwrap_or_default();
    let _ = crate::platform::storage::write_text(QUEUE_CURRENT_KEY, &cur_gd);
    let _ = crate::platform::storage::write_text(PLAY_MODE_KEY, &u.get_play_mode().to_string());
}

/// 启动恢复（setup 顶部、接线回调之前，setup 仅跑一次故无需额外标志）：
/// 读三键 → 逐项 register_full 重登记 → 重建队列模型 → 按 queue-current 的 gd_id 恢复当前曲状态
/// （只恢复状态不自动播放：不取流、playing=false、progress=0）。
fn restore_session(ui: &crate::AppWindow) {
    if let Some(m) = crate::platform::storage::read_text(PLAY_MODE_KEY)
        .and_then(|s| s.parse::<i32>().ok())
    {
        ui.set_play_mode(m);
    }
    let Some(txt) = crate::platform::storage::read_text(QUEUE_KEY) else { return; };
    let Ok(items) = serde_json::from_str::<Vec<PersistQueueItem>>(&txt) else { return; };
    if items.is_empty() {
        return;
    }
    let rows: Vec<crate::Song> = items
        .into_iter()
        .map(|it| {
            let id = gd::register_full(
                &it.source, &it.gd_id, &it.name, &it.artist, &it.album, it.duration_sec,
            );
            crate::Song {
                id,
                name: it.name.into(),
                artist: it.artist.into(),
                album: it.album.into(),
                cover: gd::cover_image(&it.cover_url),
                duration: fmt_dur(it.duration_sec).into(),
                duration_sec: it.duration_sec,
                url: it.cover_url.into(),
                liked: gd::liked_contains(id),
            }
        })
        .collect();
    ui.set_queue(Rc::new(VecModel::from(rows)).into());
    spawn_queue_covers(ui);
    // 当前曲：按 queue-current 的 gd_id 在队列中定位行，仅恢复状态不出声
    let Some(cur_gd) = crate::platform::storage::read_text(QUEUE_CURRENT_KEY) else { return; };
    if cur_gd.is_empty() {
        return;
    }
    let q = ui.get_queue();
    for i in 0..q.row_count() {
        let Some(row) = q.row_data(i) else { continue };
        let Some((_, gd_id, _, _, _, _)) = gd::lookup_full(row.id) else { continue };
        if gd_id != cur_gd {
            continue;
        }
        let liked = gd::liked_contains(row.id);
        ui.set_current_song(crate::Song {
            id: row.id,
            name: row.name.clone(),
            artist: row.artist.clone(),
            album: row.album.clone(),
            cover: gd::cover_image(&row.url.to_string()),
            duration: row.duration.clone(),
            duration_sec: row.duration_sec,
            url: row.url.clone(),
            liked,
        });
        ui.set_liked(liked);
        ui.set_playing(false);
        ui.set_progress(0.0);
        spawn_current_cover(ui);
        break;
    }
}

/// 秒 → mm:ss。
fn fmt_dur(sec: f32) -> String {
    let s = sec.max(0.0) as u64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// 解析 LRC 为 (秒, 文本) 列表。
/// 时间标签格式 [mm:ss.xx]；[ti:]/[ar:]/[al:]/[offset:] 等元数据标签（冒号前非数字）直接忽略。
fn parse_lrc(raw: &str) -> Vec<(f32, String)> {
    let mut out: Vec<(f32, String)> = Vec::new();
    for line in raw.lines() {
        let mut rest = line;
        let mut times: Vec<f32> = Vec::new();
        while let Some(open) = rest.find('[') {
            if let Some(close_off) = rest[open..].find(']') {
                let tag = rest[open + 1..open + close_off].trim();
                rest = &rest[open + close_off + 1..];
                // 仅接受 mm:ss[.xx] 两段式数字时间标签
                let mut parts = tag.split(':');
                if let (Some(mm_s), Some(ss_s), None) = (parts.next(), parts.next(), parts.next()) {
                    if let (Ok(mm), Ok(ss)) =
                        (mm_s.trim().parse::<f32>(), ss_s.trim().parse::<f32>())
                    {
                        times.push(mm * 60.0 + ss);
                    }
                }
            } else {
                break;
            }
        }
        let text = rest.trim().to_string();
        for t in times {
            out.push((t, text.clone()));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 按播放模式在队列中选上一/下一首目标索引。
/// mode: 0=列表循环(绕回) 1=单曲循环(重播当前) 2=随机(≠当前)。queue 空返回 None。
fn pick_target(u: &crate::AppWindow, dir: i32) -> Option<usize> {
    let q = u.get_queue();
    let n = q.row_count();
    if n == 0 {
        return None;
    }
    let cur_id = u.get_current_song().id;
    let mut idx = 0usize;
    for i in 0..n {
        if let Some(s) = q.row_data(i) {
            if s.id == cur_id {
                idx = i;
                break;
            }
        }
    }
    // 单曲循环 / 队列只剩一首：重播当前
    if n == 1 || u.get_play_mode() == 1 {
        return Some(idx);
    }
    match u.get_play_mode() {
        2 => {
            let rnd = RND.with(|r| {
                let mut g = r.borrow_mut();
                *g = g.wrapping_mul(1103515245).wrapping_add(12345);
                *g
            });
            let mut r = (rnd as usize) % n;
            if r == idx {
                r = (r + 1) % n;
            }
            Some(r)
        }
        _ => {
            let next = (((idx as i32 + dir) % n as i32) + n as i32) % n as i32;
            Some(next as usize)
        }
    }
}

/// 核心切歌：按 UI id 重建 current-song → recent_push → 取流播放。不改动队列顺序。
fn start_play(u: &crate::AppWindow, id: i32) {
    let Some((source, gd_id, name, artist, album, dur)) = gd::lookup_full(id) else {
        return;
    };
    let liked = gd::liked_contains(id);
    let cover_url = queue_row_url(u, id);
    u.set_current_song(crate::Song {
        id,
        name: name.clone().into(),
        artist: artist.clone().into(),
        album: album.clone().into(),
        cover: gd::cover_image(&cover_url),
        duration: fmt_dur(dur).into(),
        duration_sec: dur,
        url: cover_url.into(),
        liked,
    });
    u.set_liked(liked);
    gd::recent_push(id, &name, &artist, &album);
    u.set_playing_loading(true);
    u.set_progress(0.0);
    spawn_current_cover(u);
    let weak = u.as_weak();
    wasm_bindgen_futures::spawn_local(async move {
        match gd::song_url(&gd_id, &source, gd::DEFAULT_BR).await {
            Some(url) => {
                crate::platform::audio::play_url(&url, dur);
                if let Some(u) = weak.upgrade() {
                    u.set_playing(true);
                    u.set_playing_loading(false);
                    u.set_progress(0.0);
                }
            }
            None => {
                if let Some(u) = weak.upgrade() {
                    u.set_playing_loading(false);
                    show_toast(&u, "无法获取播放地址", "error");
                }
            }
        }
    });
    // 自动预拉歌词（填充高亮状态；不自动打开抽屉，仅 request-lyrics 才开）
    fetch_lyrics(u, id, false);
    // 当前曲变更 → 落盘队列/当前曲/播放模式
    persist_write(u);
}

/// 拉取并解析歌词：填 ui.lyrics + 写内部 LRC 状态；open_panel=true 时打开歌词抽屉。
fn fetch_lyrics(u: &crate::AppWindow, id: i32, open_panel: bool) {
    let Some((_src, gd_id, _, _, _, _)) = gd::lookup_full(id) else {
        return;
    };
    let weak = u.as_weak();
    wasm_bindgen_futures::spawn_local(async move {
        match gd::lyric(&gd_id).await {
            Some(lrc) => {
                let parsed = parse_lrc(&lrc);
                LRC.with(|c| *c.borrow_mut() = (id, parsed.clone()));
                if let Some(u) = weak.upgrade() {
                    u.set_lyrics(Rc::new(VecModel::from(
                        crate::core::lyrics::build_lyric_lines(&parsed),
                    )).into());
                    u.set_current_lyric(0);
                    if open_panel {
                        u.set_lyrics_open(true);
                        u.set_playlist_open(false);
                    }
                }
            }
            None => {
                LRC.with(|c| *c.borrow_mut() = (id, Vec::new()));
                if let Some(u) = weak.upgrade() {
                    u.set_lyrics(Rc::new(VecModel::<crate::LyricLine>::default()).into());
                    u.set_current_lyric(0);
                    if open_panel {
                        u.set_lyrics_open(true);
                        show_toast(&u, "暂无歌词", "info");
                    }
                }
            }
        }
    });
}

/// 切换本地收藏态，返回新值。
fn apply_like(id: i32) -> bool {
    if gd::liked_contains(id) {
        gd::liked_remove(id);
        false
    } else {
        gd::liked_insert(id);
        true
    }
}

/// 刷新队列模型中某行的 liked 显示。
fn sync_queue_liked(u: &crate::AppWindow, id: i32, liked: bool) {
    let q = u.get_queue();
    for i in 0..q.row_count() {
        if let Some(s) = q.row_data(i) {
            if s.id == id {
                let mut ns = s.clone();
                ns.liked = liked;
                q.set_row_data(i, ns);
                break;
            }
        }
    }
}

pub fn setup(ui: &crate::AppWindow) {
    // ---- 启动恢复（先于一切回调接线）：队列/当前曲状态/播放模式 ----
    restore_session(ui);

    // ---- 点歌：同一首 → toggle；新歌 → 建 current-song + 入队 + 取流播放 ----
    let weak = ui.as_weak();
    ui.on_play_song(move |id| {
        let Some(u) = weak.upgrade() else { return; };
        let cur_id = u.get_current_song().id;
        // 重播当前曲：仅切播放/暂停
        if id != 0 && id == cur_id {
            crate::platform::audio::toggle();
            u.set_playing(crate::platform::audio::active());
            return;
        }
        let Some((source, gd_id, name, artist, album, dur)) = gd::lookup_full(id) else {
            return;
        };
        // 队列管理：去重后插到当前播放行位置（无当前曲则 append）
        let q = u.get_queue();
        let mut list: Vec<crate::Song> =
            (0..q.row_count()).filter_map(|i| q.row_data(i)).collect();
        // 去重 retain 前保留同 id 旧行封面 URL，避免重排丢封面
        let old_url = list
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.url.to_string())
            .unwrap_or_default();
        list.retain(|s| s.id != id);
        let row = crate::Song {
            id,
            name: name.clone().into(),
            artist: artist.clone().into(),
            album: album.clone().into(),
            cover: gd::cover_image(&old_url),
            duration: fmt_dur(dur).into(),
            duration_sec: dur,
            url: old_url.into(),
            liked: gd::liked_contains(id),
        };
        if cur_id != 0 {
            let pos = list.iter().position(|s| s.id == cur_id).unwrap_or(list.len());
            list.insert(pos, row);
        } else {
            list.push(row);
        }
        u.set_queue(Rc::new(VecModel::from(list)).into());
        spawn_queue_covers(&u);
        start_play(&u, id);
    });

    // ---- 播放/暂停 ----
    let weak = ui.as_weak();
    ui.on_play_toggle(move || {
        let Some(u) = weak.upgrade() else { return; };
        crate::platform::audio::toggle();
        u.set_playing(crate::platform::audio::active());
    });

    // ---- 上一首 / 下一首 ----
    let weak = ui.as_weak();
    ui.on_prev_song(move || {
        let Some(u) = weak.upgrade() else { return; };
        let Some(idx) = pick_target(&u, -1) else { return; };
        if let Some(row) = u.get_queue().row_data(idx) {
            start_play(&u, row.id);
        }
    });
    let weak = ui.as_weak();
    ui.on_next_song(move || {
        let Some(u) = weak.upgrade() else { return; };
        let Some(idx) = pick_target(&u, 1) else { return; };
        if let Some(row) = u.get_queue().row_data(idx) {
            start_play(&u, row.id);
        }
    });

    // ---- 播放模式：UI 已持有新值，这里仅落盘持久化 ----
    let weak = ui.as_weak();
    ui.on_play_mode_changed(move || {
        if let Some(u) = weak.upgrade() {
            let _ = crate::platform::storage::write_text(
                PLAY_MODE_KEY,
                &u.get_play_mode().to_string(),
            );
        }
    });

    // ---- 进度条跳转：0..1 → 秒 ----
    let weak = ui.as_weak();
    ui.on_seek(move |f| {
        let Some(u) = weak.upgrade() else { return; };
        let f = f.clamp(0.0, 1.0);
        let dur = u.get_current_song().duration_sec;
        if dur <= 0.0 {
            return;
        }
        u.set_progress(f);
        crate::platform::audio::seek(f * dur);
    });

    // ---- 音量：门面 + 持久化（唯一写入方）----
    ui.on_volume_changed(move |v| {
        crate::platform::audio::set_volume(v);
        let _ = crate::platform::storage::write_text("volume", &v.to_string());
    });

    // ---- 收藏切换：当前曲 / 按 id ----
    let weak = ui.as_weak();
    ui.on_like_toggle(move || {
        let Some(u) = weak.upgrade() else { return; };
        let id = u.get_current_song().id;
        if id == 0 {
            return;
        }
        let now = apply_like(id);
        u.set_liked(now);
        let mut cur = u.get_current_song();
        cur.liked = now;
        u.set_current_song(cur);
        sync_queue_liked(&u, id, now);
    });
    let weak = ui.as_weak();
    ui.on_like_toggle_id(move |id| {
        let Some(u) = weak.upgrade() else { return; };
        if id == 0 {
            return;
        }
        let now = apply_like(id);
        if u.get_current_song().id == id {
            u.set_liked(now);
            let mut cur = u.get_current_song();
            cur.liked = now;
            u.set_current_song(cur);
        }
        sync_queue_liked(&u, id, now);
    });

    // ---- 清空队列 ----
    let weak = ui.as_weak();
    ui.on_clear_queue(move || {
        let Some(u) = weak.upgrade() else { return; };
        u.set_queue(Rc::new(VecModel::<crate::Song>::default()).into());
        persist_write(&u);
    });

    // ---- 移除队列行；若移除的是当前曲 → 切下一首（无则停止）----
    let weak = ui.as_weak();
    ui.on_remove_song(move |id| {
        let Some(u) = weak.upgrade() else { return; };
        let was_current = u.get_current_song().id == id;
        let q = u.get_queue();
        let list: Vec<crate::Song> = (0..q.row_count())
            .filter_map(|i| q.row_data(i))
            .filter(|s| s.id != id)
            .collect();
        u.set_queue(Rc::new(VecModel::from(list)).into());
        if was_current {
            let next =
                pick_target(&u, 1).and_then(|i| u.get_queue().row_data(i)).map(|s| s.id);
            match next {
                Some(nid) => start_play(&u, nid),
                None => {
                    crate::platform::audio::stop();
                    u.set_playing(false);
                    u.set_progress(0.0);
                }
            }
        }
        persist_write(&u);
    });

    // ---- 请求歌词：解析 LRC 填 lyrics 并打开抽屉 ----
    let weak = ui.as_weak();
    ui.on_request_lyrics(move |id| {
        let Some(u) = weak.upgrade() else { return; };
        fetch_lyrics(&u, id, true);
    });

    // ---- Hero「播放全部」：hero-songs 重建队列 → 播第一首 ----
    let weak = ui.as_weak();
    ui.on_hero_play_all(move || {
        let Some(u) = weak.upgrade() else { return; };
        let hero = u.get_hero_songs();
        let mut songs: Vec<crate::Song> = Vec::new();
        for i in 0..hero.row_count() {
            let Some(h) = hero.row_data(i) else { continue; };
            if let Some((_s, _g, name, artist, album, dur)) = gd::lookup_full(h.id) {
                songs.push(crate::Song {
                    id: h.id,
                    name: name.into(),
                    artist: artist.into(),
                    album: album.into(),
                    cover: gd::cover_image(""),
                    duration: fmt_dur(dur).into(),
                    duration_sec: dur,
                    url: "".into(),
                    liked: gd::liked_contains(h.id),
                });
            }
        }
        if songs.is_empty() {
            show_toast(&u, "暂无可播放的歌曲", "info");
            return;
        }
        let first = songs[0].id;
        u.set_queue(Rc::new(VecModel::from(songs)).into());
        spawn_queue_covers(&u);
        start_play(&u, first);
    });

    // ---- 详情页「播放全部」：detail-songs 重建队列 → 播第一首 ----
    let weak = ui.as_weak();
    ui.on_detail_play_all(move || {
        let Some(u) = weak.upgrade() else { return; };
        let m = u.get_detail_songs();
        let songs: Vec<crate::Song> = (0..m.row_count()).filter_map(|i| m.row_data(i)).collect();
        if songs.is_empty() {
            show_toast(&u, "暂无可播放的歌曲", "info");
            return;
        }
        let first = songs[0].id;
        u.set_queue(Rc::new(VecModel::from(songs.clone())).into());
        spawn_queue_covers(&u);
        start_play(&u, first);
    });

    // ---- 抽屉打开时定位正在播放行 ----
    let weak = ui.as_weak();
    ui.on_locate_playing(move || {
        let Some(u) = weak.upgrade() else { return; };
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
            let w = weak.clone();
            slint::Timer::single_shot(Duration::from_millis(80), move || {
                if let Some(u) = w.upgrade() {
                    u.set_playlist_scroll_index(idx);
                }
            });
        }
    });

    // ---- web 无迷你队列：空接 ----
    ui.on_mini_queue_toggle(|| {});

    // ---- 周期轮询（250ms）：进度 / playing 同步、歌词高亮、播完自动切歌 ----
    let tick_weak = ui.as_weak();
    let timer = Rc::new(slint::Timer::default());
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        let Some(u) = tick_weak.upgrade() else { return; };
        let (pos, playing) = crate::platform::audio::status();
        let dur = u.get_current_song().duration_sec;
        if dur > 0.0 {
            u.set_progress((pos / dur).clamp(0.0, 1.0));
        }
        u.set_playing(playing);
        // 正常出声即复位 finished 消费标记
        if playing {
            FINISHED_SEEN.with(|f| *f.borrow_mut() = false);
        }
        // 歌词高亮行（仅当 LRC 属于当前曲）
        let cur_id = u.get_current_song().id;
        let li = LRC.with(|c| {
            let g = c.borrow();
            if g.0 != cur_id {
                -1
            } else {
                let mut idx = 0i32;
                for (i, (t, _)) in g.1.iter().enumerate() {
                    if *t <= pos {
                        idx = i as i32;
                    } else {
                        break;
                    }
                }
                idx
            }
        });
        if li >= 0 {
            u.set_current_lyric(li);
        }
        // 逐词歌词：推送当前播放毫秒，驱动行内进度填充
        u.set_current_time_ms((pos * 1000.0) as i32);
        // 播完自动切歌：onended 只触发一次；取流 loading 期间不抢切
        if crate::platform::audio::finished() && !playing && !u.get_playing_loading() {
            let consumed = FINISHED_SEEN.with(|f| *f.borrow());
            if !consumed {
                FINISHED_SEEN.with(|f| *f.borrow_mut() = true);
                let mode = u.get_play_mode();
                if mode == 1 {
                    // 单曲循环：重播当前
                    let id = u.get_current_song().id;
                    if id != 0 {
                        start_play(&u, id);
                    }
                } else if let Some(idx) = pick_target(&u, 1) {
                    if let Some(row) = u.get_queue().row_data(idx) {
                        start_play(&u, row.id);
                    }
                } else {
                    u.set_playing(false);
                    u.set_progress(0.0);
                }
            }
        }
    });
    std::mem::forget(timer);
}
