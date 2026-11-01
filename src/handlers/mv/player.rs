//! MV 播放器生命周期 handler：open-resource-detail(4,id) → 启动播放器 → feed Slint 帧。
//! P0 骨架：先打通 URL 请求 + 播放器启动 + 帧订阅，后续再补音频互斥/清晰度切换。

use crate::handlers::Ctx;
use super::queue;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::core::mv_player::MvPlayer;
use crate::core::mv_frame::MvFrame;
use crate::core::mv_frame_convert::mv_frame_to_image;
use crate::core::mv_state::MvPlaybackState;
use crate::core::mv_event::MvPlaybackEvent;
use crate::AppWindow;
use slint::ComponentHandle;
use tokio::sync::watch;

/// 全局 MV 播放器实例。
static MV_PLAYER: OnceLock<Mutex<Option<MvPlayer>>> = OnceLock::new();

/// 播放器实例代数：换曲 / 切清晰度时自增。
/// 旧实例残留的帧/水位/事件订阅任务发现代数不符就静默退出，
/// 避免旧画面覆盖新画面（旧解码线程退出前还会再推几帧）。
static MV_FRAME_GEN: AtomicU64 = AtomicU64::new(0);

/// 清晰度切换令牌：连续点击时只有最后一次生效，旧任务完成预热后自动作废。
static MV_SWITCH_TOKEN: AtomicU64 = AtomicU64::new(0);

/// 清晰度（0=480p 1=720p 2=1080p）。跨 MV 记忆，默认 1080p。
static MV_QUALITY: AtomicU32 = AtomicU32::new(2);
/// 音量（0-100，跨 MV 记忆）+ 最近一次非零音量（静音恢复用）。
static MV_VOLUME: AtomicU32 = AtomicU32::new(100);
static MV_LAST_VOLUME: AtomicU32 = AtomicU32::new(100);
/// 当前播放的 MV（id, title, artist）——清晰度切换时原地重开用。
static MV_CURRENT: Mutex<Option<(i64, String, String)>> = Mutex::new(None);

/// 连续播放失败次数（取流地址失败/解码失败）：整条队列都失败才退出播放器。
static MV_FAIL_STREAK: AtomicU32 = AtomicU32::new(0);
/// 最近一次渲染帧的时间戳（微秒）：网络断流重连后恢复播放位置用。
static MV_LAST_POS_US: AtomicU64 = AtomicU64::new(0);
/// 待恢复的播放位置（微秒）：重连当前 MV 成功后一次性 seek 到该点。
static MV_PENDING_SEEK_US: AtomicU64 = AtomicU64::new(0);
/// 当前 MV 连续重连次数：网络断流自动原地重连用，超限才走"跳下一首"。
static MV_RETRY_STREAK: AtomicU32 = AtomicU32::new(0);

fn get_player() -> &'static Mutex<Option<MvPlayer>> {
    MV_PLAYER.get_or_init(|| Mutex::new(None))
}

/// 清晰度索引 → mv_url 的 r 参数。
fn quality_r(idx: u32) -> u32 {
    [480, 720, 1080][idx.min(2) as usize]
}

/// 启动时恢复上次选择的 MV 清晰度（config.toml 的 play.mv-quality-index）。
pub fn init_quality(idx: u32) {
    MV_QUALITY.store(idx.min(2), Ordering::Relaxed);
}

/// 持久化清晰度选择：下次启动与后续 MV 继续沿用（仅在值变化时落盘）。
fn persist_quality(idx: u32) {
    let mut cfg = crate::persist::config::AppConfig::load();
    if cfg.play.mv_quality_index != idx as i32 {
        cfg.play.mv_quality_index = idx as i32;
        cfg.save();
    }
}

/// 打开 MV 全屏播放器。kind=4 为 MV。
/// 完整 Ctx 版本（含 toast/rt 等）。
pub fn open_mv_player(ctx: &Ctx, mv_id: i64, title: String, artist: String) {
    open_mv_player_internal(ctx.ui.clone(), ctx.client.clone(), ctx.rt.clone(), mv_id, title, artist, None);
}

/// 轻量版本：只传 Weak<AppWindow> + Client + rt，供 detail.rs 的拦截路由使用
/// （那里只有这三个句柄，没有完整 Ctx）。
pub fn open_mv_player_from_ids(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    mv_id: i64,
    title: String,
    artist: String,
) {
    open_mv_player_internal(ui.clone(), client.clone(), rt.clone(), mv_id, title, artist, None);
}

/// 播放本地视频文件（从本地资源页入口）。
/// 直接用 MV 播放器播放本地路径（ffmpeg 直读文件，不走网络代理）。
/// 不参与 MV 队列，不预缓冲下一首。
pub fn open_local_video(
    ui: &slint::Weak<AppWindow>,
    _client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    path: String,
    title: String,
) {
    // 启用文件拖拽处理：仅本地资源入口打开的视频播放器支持拖入视频文件播放
    #[cfg(target_os = "windows")]
    crate::platform::drop_files::set_enabled(true);

    // 1. 停掉旧播放器
    let gen_id = MV_FRAME_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    MV_SWITCH_TOKEN.fetch_add(1, Ordering::Relaxed);
    MV_LAST_POS_US.store(0, Ordering::Relaxed);
    if let Ok(mut g) = get_player().lock() {
        if let Some(mut p) = g.take() {
            p.stop_playback();
        }
    }
    crate::handlers::mv::window::pause_rodio();

    let ui_weak = ui.clone();
    let rt_clone = rt.clone();
    rt_clone.spawn(async move {
        let cfg = crate::core::mv_config::MvPlaybackConfig::default();
        let mut player = MvPlayer::with_config(cfg);
        let frame_rx = player.frame_buffer.subscribe();
        let events_rx = match player.start_playback(&path) {
            Ok(rx) => rx,
            Err(e) => {
                log::error!(target: "mv", "local video start failed: {e}");
                return;
            }
        };

        player.set_volume(MV_VOLUME.load(Ordering::Relaxed));
        spawn_buffered_reporter(ui_weak.clone(), player.state(), gen_id);

        if let Ok(mut g) = get_player().lock() {
            *g = Some(player);
        }

        spawn_frame_feeder(ui_weak.clone(), frame_rx, gen_id);
        // 本地视频不参与队列连播，Ended 仅关闭播放器
        spawn_event_forwarder_local(ui_weak.clone(), events_rx, gen_id);

        let title_show = title.clone();
        let ui_show = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_show.upgrade() {
                u.set_mv_notice("".into());
                u.set_mv_show_search_box(false); // 本地视频不显示搜索框
                u.set_mv_title(title_show.into());
                u.set_mv_artist("本地视频".into());
                u.set_mv_position(0);
                u.set_mv_buffered(0);
                u.set_mv_playing(true);
                u.set_mv_quality(MV_QUALITY.load(Ordering::Relaxed) as i32);
                u.set_mv_switching(false);
                u.set_mv_buffering(false);
                u.set_mv_volume(MV_VOLUME.load(Ordering::Relaxed) as i32);
                u.set_mv_fullscreen(false);
                u.set_mv_player_visible(true);
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn open_mv_player_internal(
    ui_weak: slint::Weak<AppWindow>,
    client: Arc<crate::network::netease::client::NeteaseClient>,
    rt: tokio::runtime::Handle,
    mv_id: i64,
    title: String,
    artist: String,
    // 过渡提示（如"上一首无法播放，已切换到当前这首"）：播放器真正显示时一次性呈现。
    // 用参数传递而不是全局队列，避免用户手动切歌后残留旧提示。
    notice: Option<String>,
) {
    // 网络 MV 播放器不支持本地文件拖拽，禁用拖拽处理
    #[cfg(target_os = "windows")]
    crate::platform::drop_files::set_enabled(false);

    // 1. 先停掉旧播放器（代数自增：旧实例的订阅任务随之失效）
    let gen_id = MV_FRAME_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    // 作废在途的清晰度切换任务（它们不得再替换播放器）
    MV_SWITCH_TOKEN.fetch_add(1, Ordering::Relaxed);
    // 非重连的打开（用户切歌/新开 MV）：清零断流恢复基准，避免沿用上一首的进度
    if MV_PENDING_SEEK_US.load(Ordering::Relaxed) == 0 {
        MV_LAST_POS_US.store(0, Ordering::Relaxed);
    }
    if let Ok(mut g) = get_player().lock() {
        if let Some(mut p) = g.take() {
            p.stop_playback();
        }
    }

    // 1.5 音频互斥：MV 有声音，暂停音乐播放避免叠加
    crate::handlers::mv::window::pause_rodio();

    // 1.6 队列定位：命中则设为当前；未命中时替换为单曲队列（保证循环与当前播放一致）
    if let Ok(mut q) = queue::queue().lock() {
        if !q.set_current_by_id(mv_id) {
            q.set_items(vec![queue::MvQueueItem {
                id: mv_id, name: title.clone(), artist: artist.clone(),
                cover_url: String::new(), duration: String::new(),
                from: 0xFF333333, to: 0xFF222222,
            }], 0);
        }
    }

    // 记录当前 MV（清晰度切换时重开）
    if let Ok(mut c) = MV_CURRENT.lock() {
        *c = Some((mv_id, title.clone(), artist.clone()));
    }

    let rt_open = rt.clone();
    rt_open.spawn(async move {
        // 2. 取流地址：优先 mv/detail 的 brs（清晰度 → 地址 map）按用户所选清晰度精确命中，
        //    失败才回退 mv_url。绝不像旧逻辑那样盲用 data.url（服务端默认推荐通常是最高码率），
        //    否则下一个 MV 会播放默认清晰度，而按钮仍显示记忆值。
        let idx = MV_QUALITY.load(Ordering::Relaxed);
        let mut urls: Vec<String> = Vec::new();
        match client.mv_detail(mv_id as u64).await {
            Ok(detail) => {
                if let Some(u) = pick_detail_url(&detail, idx) { urls.push(u); }
            }
            Err(e) => log::warn!(target: "mv", "mv_detail failed: {e}"),
        }
        if urls.is_empty() {
            let resp = client.mv_url(mv_id as u64, Some(quality_r(idx))).await;
            match pick_urls_for_quality(resp, idx) {
                Ok(u) => urls = u,
                Err(e) => {
                    log::error!(target: "mv", "mv_url failed: {e}");
                    advance_on_failure(&ui_weak, &client, &rt, "取流地址失败");
                    return;
                }
            }
        }

        // 3. 依次尝试候选地址（data.url + 多码率 brs）→ 换源兜底
        // 输出尺寸保持源分辨率（cfg 默认 target=0）：清晰度只由所选流决定，
        // 统一缩到 720p 会让 480/720/1080 三档看起来完全一样。
        let cfg = crate::core::mv_config::MvPlaybackConfig::default();
        let mut started: Option<(MvPlayer, watch::Receiver<Option<Arc<MvFrame>>>, tokio::sync::mpsc::UnboundedReceiver<MvPlaybackEvent>)> = None;
        for (i, url) in urls.iter().enumerate() {
            let mut player = MvPlayer::with_config(cfg);
            let frame_rx = player.frame_buffer.subscribe();
            match player.start_playback(url) {
                Ok(events_rx) => {
                    log::info!(target: "mv", "stream started with candidate {}/{}", i + 1, urls.len());
                    started = Some((player, frame_rx, events_rx));
                    break;
                }
                Err(e) => log::warn!(target: "mv", "candidate {}/{} start failed: {e}", i + 1, urls.len()),
            }
        }
        let Some((player, frame_rx, events_rx)) = started else {
            log::error!(target: "mv", "all {} stream candidates failed", urls.len());
            advance_on_failure(&ui_weak, &client, &rt, "所有流地址均不可用");
            return;
        };

        // 3.4 网络断流重连的恢复点：重连成功后 seek 回中断位置
        let resume_us = MV_PENDING_SEEK_US.swap(0, Ordering::Relaxed);
        if resume_us > 0 {
            log::info!(target: "mv", "resume playback at {}ms after reconnect", resume_us / 1000);
            player.seek_to_us(resume_us);
        }

        // 3.5 应用记忆音量
        player.set_volume(MV_VOLUME.load(Ordering::Relaxed));

        // 3.6 预加载水位上报（每 250ms 轮询一次：demux 已读入的时间点 → 进度条"已缓冲"区间）
        spawn_buffered_reporter(ui_weak.clone(), player.state(), gen_id);

        // 4. 保存播放器实例
        if let Ok(mut g) = get_player().lock() {
            *g = Some(player);
        }

        // 5. 帧通道订阅（独立 task，不停 feed Slint）
        spawn_frame_feeder(ui_weak.clone(), frame_rx, gen_id);

        // 6. 事件通道订阅（Duration/Ended/Error；Ended 自动播放下一首）
        spawn_event_forwarder(ui_weak.clone(), events_rx, client.clone(), rt.clone(), gen_id);

        // 6.5 预缓冲下一首 MV：后台取下一首的流地址并启动代理预取分片，
        //     当前 MV 播完切到下一首时直接复用代理，实现"秒开 + 已预缓冲"。
        let pb_client = client.clone();
        let pb_rt = rt.clone();
        pb_rt.spawn(async move {
            let next_id = {
                let q = queue::queue().lock().ok();
                q.and_then(|g| g.peek_next().map(|it| it.id))
            };
            let Some(next_id) = next_id else { return };
            let idx = MV_QUALITY.load(Ordering::Relaxed);
            // 取下一首的流地址（与当前 MV 同清晰度）
            let mut next_url: Option<String> = None;
            if let Ok(detail) = pb_client.mv_detail(next_id as u64).await {
                next_url = pick_detail_url(&detail, idx);
            }
            if next_url.is_none() {
                let resp = pb_client.mv_url(next_id as u64, Some(quality_r(idx))).await;
                if let Ok(urls) = pick_urls_for_quality(resp, idx) {
                    next_url = urls.into_iter().next();
                }
            }
            if let Some(url) = next_url {
                crate::core::mv_fetch::pre_buffer_next(&url);
            }
        });

        // 7. 显示全屏播放器（Rust 回调）
        // 注意：本闭包外的 UI 属性 set 一律不允许——open_mv_player_from_ids 路径
        // 运行在 tokio worker 线程，非 UI 线程 upgrade/set 会静默失败。
        // 失败自动跳转的过渡提示与播放器首帧同时呈现（空串则清掉上一次残留提示）。
        let notice_show = notice.unwrap_or_default();
        let ui_show = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_show.upgrade() {
                u.set_mv_notice(notice_show.into());
                // MV 播放器抽屉统一显示搜索框（搜索只影响抽屉内容，不改变当前播放队列）
                u.set_mv_show_search_box(true);
                u.set_mv_search_active(false);
                u.set_mv_search_text("".into());
                u.set_mv_title(title.clone().into());
                u.set_mv_artist(artist.clone().into());
                u.set_mv_position(0);
                u.set_mv_buffered(0);
                u.set_mv_playing(true);
                u.set_mv_quality(MV_QUALITY.load(Ordering::Relaxed) as i32);
                u.set_mv_switching(false);   // 清掉上一次切换残留的进度胶囊
                u.set_mv_buffering(false);   // 清掉上一次缓冲残留的加载指示
                u.set_mv_volume(MV_VOLUME.load(Ordering::Relaxed) as i32);
                u.set_mv_fullscreen(false);
                queue::sync_ui(&u);
                u.set_mv_player_visible(true);
                // 记录最近播放的 MV（kind=4），供最近播放页 MV 分类展示
                crate::persist::recent::record(
                    mv_id,
                    title.clone(),
                    artist.clone(),
                    String::new(),
                    String::new(),
                    String::new(),
                    0.0,
                    4,
                );
            }
        });
    });
}

/// 帧订阅任务：把播放器最新帧投递到 UI 线程（进度也由帧 PTS 驱动）。
/// gen 不符（播放器已被替换）时丢弃，避免旧实例的残留帧覆盖新画面。
/// 用"最多一帧在途"的闸门：UI 渲染跟不上时直接跳过旧帧（watch 里永远是最新帧），
/// 否则事件队列会持续积压未渲染的帧，表现为画面越来越滞后于声音（滞留感）。
fn spawn_frame_feeder(
    ui_weak: slint::Weak<AppWindow>,
    mut rx: watch::Receiver<Option<Arc<MvFrame>>>,
    gen_id: u64,
) {
    tokio::task::spawn(async move {
        let in_flight = Arc::new(AtomicBool::new(false));
        while rx.changed().await.is_ok() {
            let frame = rx.borrow_and_update().clone();
            let Some(arc_frame) = frame else { continue };
            if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { continue; }
            // 首帧到达 = 本次播放成功，清零连续失败计数（失败跳转链到此终止）
            MV_FAIL_STREAK.store(0, Ordering::Relaxed);
            // 播放有进展：重连计数清零，并记录最新位置（断流重连恢复用）
            MV_RETRY_STREAK.store(0, Ordering::Relaxed);
            MV_LAST_POS_US.store(arc_frame.ts_us, Ordering::Relaxed);
            if in_flight.load(Ordering::Relaxed) { continue; }
            in_flight.store(true, Ordering::Relaxed);
            let ui = ui_weak.clone();
            let flag = in_flight.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if MV_FRAME_GEN.load(Ordering::Relaxed) == gen_id {
                    if let Some(u) = ui.upgrade() {
                        u.set_mv_position((arc_frame.ts_us / 1000) as i32);
                        let img = mv_frame_to_image(&arc_frame);
                        u.set_mv_video_frame(img);
                    }
                }
                flag.store(false, Ordering::Relaxed);
            });
        }
    });
}

/// 预加载水位上报：每 250ms 把 demux 已读入的时间点写给 UI（进度条"已缓冲"区间）。
fn spawn_buffered_reporter(ui_weak: slint::Weak<AppWindow>, state: Arc<MvPlaybackState>, gen_id: u64) {
    tokio::task::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            tick.tick().await;
            if state.shutdown() || MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { break; }
            let ms = (state.buffered_us() / 1000) as i32;
            let buffering = state.buffering();
            let ui = ui_weak.clone();
            let sent = slint::invoke_from_event_loop(move || {
                if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { return; }
                if let Some(u) = ui.upgrade() {
                    u.set_mv_buffered(ms);
                    // 缓冲门控状态镜像到 UI（播放器中央显示"正在缓冲"加载指示）
                    u.set_mv_buffering(buffering);
                }
            });
            if sent.is_err() { break; }
        }
    });
}

/// 事件订阅：Duration/Error 进 UI；Ended 自动播下一首。gen 不符则全部忽略。
fn spawn_event_forwarder(
    ui_weak: slint::Weak<AppWindow>,
    events_rx: tokio::sync::mpsc::UnboundedReceiver<MvPlaybackEvent>,
    client: Arc<crate::network::netease::client::NeteaseClient>,
    rt: tokio::runtime::Handle,
    gen_id: u64,
) {
    tokio::task::spawn(async move {
        tokio::pin!(events_rx);
        while let Some(ev) = events_rx.recv().await {
            if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { continue; }
            // Ended 在 tokio task 内直接处理（自动下一首），不进 UI 线程
            if matches!(ev, MvPlaybackEvent::Ended) {
                log::info!(target: "mv", "playback ended, auto next");
                play_neighbor(&ui_weak, &client, &rt, true);
                continue;
            }
            // 播放出错：区分"网络断流"与"源/解码失败"。
            // 网络断流（demux 上报的"读取流中断"）→ 原地重连当前 MV 并恢复进度，
            // 绝不直接跳到下一首（用户反馈"播着播着被强制跳下一个 MV 完全不合理"）。
            if let MvPlaybackEvent::Error(msg) = &ev {
                log::error!(target: "mv", "playback error: {msg}");
                if msg.contains("读取流中断") && retry_current_after_network_drop(&ui_weak, &client, &rt) {
                    continue;
                }
                advance_on_failure(&ui_weak, &client, &rt, "播放出错");
                continue;
            }
            let ui = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { return; }
                if let Some(u) = ui.upgrade() {
                    if let MvPlaybackEvent::Duration(us) = ev {
                        u.set_mv_duration((us / 1000) as i32);
                    }
                }
            });
        }
    });
}

/// 本地视频的事件转发：Ended 时关闭播放器（不自动下一首）。
fn spawn_event_forwarder_local(
    ui_weak: slint::Weak<AppWindow>,
    events_rx: tokio::sync::mpsc::UnboundedReceiver<MvPlaybackEvent>,
    gen_id: u64,
) {
    tokio::task::spawn(async move {
        tokio::pin!(events_rx);
        while let Some(ev) = events_rx.recv().await {
            if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { continue; }
            if matches!(ev, MvPlaybackEvent::Ended) {
                log::info!(target: "mv", "local video ended, close player");
                close_mv_player();
                let ui = ui_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui.upgrade() {
                        u.set_mv_player_visible(false);
                    }
                });
                continue;
            }
            if let MvPlaybackEvent::Error(msg) = &ev {
                log::error!(target: "mv", "local video error: {msg}");
                continue;
            }
            let ui = ui_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if MV_FRAME_GEN.load(Ordering::Relaxed) != gen_id { return; }
                if let Some(u) = ui.upgrade() {
                    if let MvPlaybackEvent::Duration(us) = ev {
                        u.set_mv_duration((us / 1000) as i32);
                    }
                }
            });
        }
    });
}

/// 等待一帧时间点抵达目标（seek 到位）；target_us=0 时任意首帧即可。
async fn mv_wait_frame_near(mut rx: watch::Receiver<Option<Arc<MvFrame>>>, target_us: u64) -> bool {
    const TOL_US: u64 = 800_000;
    loop {
        if rx.changed().await.is_err() { return false; }
        let frame = rx.borrow_and_update().clone();
        if let Some(f) = frame {
            if target_us == 0 || f.ts_us.saturating_add(TOL_US) >= target_us {
                return true;
            }
        }
    }
}

/// 清晰度切换失败：记住的清晰度回滚到 prev，UI 同步回滚并提示。
fn quality_switch_failed(ui: &slint::Weak<AppWindow>, prev: u32, msg: &str) {
    MV_QUALITY.store(prev, Ordering::Relaxed);
    let ui = ui.clone();
    let msg = msg.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(u) = ui.upgrade() {
            u.set_mv_quality(prev as i32);
            u.set_mv_switching(false);
            u.set_mv_hint(msg.into());
        }
    });
}

/// 各清晰度对应的目标码率（bps，用于从 brs 数组里挑最接近且不超过的档位）。
const BR_TARGETS: [i64; 3] = [1_500_000, 3_000_000, 6_000_000];

/// 从 mv/detail 的 brs（清晰度 → 地址 map，key 形如 "480"/"720"/"1080"）按目标清晰度精确取地址。
/// 精确档不存在时取不超过目标的最高档；全都高于目标则取最低档。
fn pick_detail_url(detail: &serde_json::Value, idx: u32) -> Option<String> {
    if let Some(brs) = detail.pointer("/data/brs").and_then(|b| b.as_object()) {
        let mut list: Vec<(u32, String)> = brs.iter().filter_map(|(k, v)| {
            let q = k.trim_end_matches(['p', 'P']).parse::<u32>().ok()?;
            let u = v.as_str()?;
            if u.is_empty() { None } else { Some((q, u.to_string())) }
        }).collect();
        if list.is_empty() { return None; }
        list.sort_by_key(|(q, _)| *q);
        let target = [480u32, 720, 1080][idx.min(2) as usize];
        return list.iter().filter(|(q, _)| *q <= target).last()
            .or_else(|| list.first()).map(|(_, u)| u.clone());
    }
    // 兼容数组格式 { brs: [{ br, url }] }：复用码率阈值选择
    pick_urls_for_quality(Ok(detail.clone()), idx).ok().and_then(|v| v.into_iter().next())
}

/// 从 mv_url 响应按用户所选清晰度排序候选地址：
/// 1) 不超过目标码率的最高档 → 2) 最低码率 → 3) 其余码率升序 → 4) data.url 最后兜底。
fn pick_urls_for_quality(resp: Result<serde_json::Value, String>, idx: u32) -> Result<Vec<String>, String> {
    let v = resp?;
    let data = v.get("data").ok_or_else(|| "mv_url: data missing".to_string())?;
    let target = BR_TARGETS[idx.min(2) as usize];
    let mut brs: Vec<(i64, String)> = data.get("brs").and_then(|b| b.as_array())
        .map(|arr| arr.iter().filter_map(|b| {
            let br = b.get("br").and_then(|x| x.as_i64()).unwrap_or(0);
            let u = b.get("url").and_then(|x| x.as_str()).unwrap_or("");
            if u.is_empty() { None } else { Some((br, u.to_string())) }
        }).collect())
        .unwrap_or_default();
    brs.sort_by_key(|(br, _)| *br);   // 升序：首个 = 最低码率，末尾 = 最高码率

    fn push(out: &mut Vec<String>, u: &str) {
        if !out.iter().any(|x| x == u) { out.push(u.to_string()); }
    }
    let mut out: Vec<String> = Vec::new();
    if let Some((_, u)) = brs.iter().filter(|(br, _)| *br <= target).last() { push(&mut out, u); }
    if let Some((_, u)) = brs.first() { push(&mut out, u); }
    for (_, u) in brs.iter() { push(&mut out, u); }
    if let Some(u) = data.get("url").and_then(|u| u.as_str()) {
        if !u.is_empty() { push(&mut out, u); }
    }
    if out.is_empty() {
        return Err("mv_url: data.url and data.brs both missing".into());
    }
    Ok(out)
}

/// 网络断流后的原地重连：重新打开当前 MV 并 seek 回中断位置。
/// 返回 true = 已发起重连；false = 不适用（重连次数超限/尚未有效播放/无当前 MV），
/// 调用方应退回 advance_on_failure 的跳下一首逻辑。
fn retry_current_after_network_drop(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
) -> bool {
    const MAX_RETRY: u32 = 3;
    let streak = MV_RETRY_STREAK.fetch_add(1, Ordering::Relaxed) + 1;
    if streak > MAX_RETRY {
        log::warn!(target: "mv", "network reconnect streak {streak} exceeded, give up retrying current");
        return false;
    }
    // 已播放出画面（有进度）才值得原地重连；一开始就断流多半是源问题，走失败跳转
    let pos_us = MV_LAST_POS_US.load(Ordering::Relaxed);
    if pos_us < 3_000_000 {
        log::warn!(target: "mv", "stream dropped before effective playback ({}ms), skip reconnect", pos_us / 1000);
        return false;
    }
    let Some((id, title, artist)) = MV_CURRENT.lock().ok().and_then(|c| c.clone()) else {
        return false;
    };
    log::info!(target: "mv", "network dropped, reconnect current MV {id} ({streak}/{MAX_RETRY}), resume at {}ms", pos_us / 1000);
    MV_PENDING_SEEK_US.store(pos_us, Ordering::Relaxed);
    let notice = Some(format!("网络中断，正在重连（第 {streak} 次）…"));
    open_mv_player_internal(ui.clone(), client.clone(), rt.clone(), id, title, artist, notice);
    true
}

/// 播放失败处理：跳到下一首继续（不退出播放器）；整条队列都失败才退出。
/// streak 在首个视频帧到达时清零（见 spawn_frame_feeder）。
fn advance_on_failure(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    reason: &str,
) {
    let streak = MV_FAIL_STREAK.fetch_add(1, Ordering::Relaxed) + 1;
    let qlen = queue::queue().lock().map(|q| q.len()).unwrap_or(0) as u32;
    let limit = qlen.clamp(1, 5);
    log::warn!(target: "mv", "play failed ({reason}), streak {streak}/{limit}");
    if streak > limit {
        // 整条队列都播不了：退出播放器并提示
        MV_FAIL_STREAK.store(0, Ordering::Relaxed);
        let ui = ui.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui.upgrade() {
                u.set_mv_player_visible(false);
                crate::handlers::ToastCtx { weak: u.as_weak() }
                    .show("MV 无法播放，已停止", "error");
            }
        });
        return;
    }
    // 提示后自动切下一首。
    // 不能用 Toast——全屏播放器覆盖层在 Toast 之上，跳转后 Toast 会被完全遮住，
    // 用户只会看到"点的是 A、播的是 B"而毫无说明。改为把提示带进播放器（mv-notice），
    // 并预告即将播放的条目名称。
    let (failed, upcoming) = queue::queue().lock().ok().map(|q| {
        (
            q.get_current().map(|it| it.name.clone()).unwrap_or_default(),
            q.peek_next().map(|it| it.name.clone()).unwrap_or_default(),
        )
    }).unwrap_or_default();
    let notice = if upcoming.is_empty() {
        format!("「{failed}」无法播放，已自动跳到下一首")
    } else {
        format!("「{failed}」无法播放，已自动切换到下一首：{upcoming}")
    };
    log::warn!(target: "mv", "notify in player: {notice}");
    play_neighbor_with_notice(ui, client, rt, true, Some(notice));
}

/// 播放队列中相邻条目（next=true 下一首，false 上一首）。
/// 供 prev/next 回调与 Ended 自动连播使用。
pub fn play_neighbor(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    next: bool,
) {
    play_neighbor_with_notice(ui, client, rt, next, None);
}

/// 带过渡提示的相邻切换（失败自动跳转用：提示会随播放器一起呈现）。
fn play_neighbor_with_notice(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    next: bool,
    notice: Option<String>,
) {
    let item = queue::queue().lock().ok().and_then(|mut q| {
        if next { q.next().cloned() } else { q.prev().cloned() }
    });
    match item {
        Some(it) => {
            log::info!(target: "mv", "play {} -> {} ({})", if next { "next" } else { "prev" }, it.name, it.id);
            open_mv_player_internal(
                ui.clone(), client.clone(), rt.clone(), it.id, it.name, it.artist, notice,
            );
        }
        None => log::info!(target: "mv", "queue empty, nothing to play"),
    }
}

/// 退出 MV 播放器（关闭全屏）。
pub fn close_mv_player() {
    // 关闭播放器时禁用文件拖拽处理
    #[cfg(target_os = "windows")]
    crate::platform::drop_files::set_enabled(false);

    if let Ok(mut g) = get_player().lock() {
        if let Some(mut p) = g.take() {
            p.stop_playback();
        }
    }
    // 关闭播放器时丢弃预缓冲的下一首代理（不再需要）
    crate::core::mv_fetch::clear_pre_buffer();
}

/// 播放队列指定条目（播放列表抽屉点选）。入参为真实 MV id（UI 侧传 entry.id——
/// Slint for 循环回调闭包的 index 绑定不可靠，见 queue.rs play_id 注释）。
pub fn play_id(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    id: i32,
) {
    let item = queue::queue().lock().ok().and_then(|mut q| q.play_id(id as i64).cloned());
    match item {
        Some(it) => {
            log::info!(target: "mv", "play id {} -> {} ({})", id, it.name, it.id);
            open_mv_player_internal(ui.clone(), client.clone(), rt.clone(), it.id, it.name, it.artist, None);
        }
        None => log::warn!(target: "mv", "play id {} not in queue", id),
    }
}

/// 播放/暂停切换。
pub fn toggle_play_pause() {
    if let Ok(g) = get_player().lock() {
        if let Some(p) = g.as_ref() {
            p.toggle_pause();
        }
    }
}

/// 跳到指定毫秒位置。
pub fn seek_to_ms(ms: i64) {
    log::info!(target: "mv", "seek request: {}ms", ms);
    if let Ok(g) = get_player().lock() {
        if let Some(p) = g.as_ref() {
            p.seek_to_us((ms * 1000) as u64);
        } else {
            log::warn!(target: "mv", "seek ignored: no active player");
        }
    } else {
        log::warn!(target: "mv", "seek ignored: player mutex poisoned");
    }
}

/// 设置音量 0-100（滑条/键盘共用）。写回 UI + hint 反馈。
pub fn set_volume_ui(ui: &slint::Weak<AppWindow>, vol: i32) {
    let v = vol.clamp(0, 100) as u32;
    apply_volume(ui, v, format!("音量 {}%", v));
}

/// 静音切换：非零 → 0；零 → 恢复最近非零音量。
pub fn mute_toggle_ui(ui: &slint::Weak<AppWindow>) {
    let cur = MV_VOLUME.load(Ordering::Relaxed);
    if cur == 0 {
        let v = MV_LAST_VOLUME.load(Ordering::Relaxed).max(1);
        apply_volume(ui, v, format!("音量 {}%", v));
    } else {
        apply_volume(ui, 0, "静音".into());
    }
}

fn apply_volume(ui: &slint::Weak<AppWindow>, v: u32, hint: String) {
    MV_VOLUME.store(v, Ordering::Relaxed);
    if v > 0 {
        MV_LAST_VOLUME.store(v, Ordering::Relaxed);
    }
    if let Ok(g) = get_player().lock() {
        if let Some(p) = g.as_ref() {
            p.set_volume(v);
        }
    }
    if let Some(u) = ui.upgrade() {
        u.set_mv_volume(v as i32);
        u.set_mv_hint(hint.into());
    }
}

/// 清晰度切换（0=480p 1=720p 2=1080p）：无缝切换。
/// 旧播放器继续播放（画面/声音/进度不中断），后台静音预热新码率流并 seek
/// 到当前位置，首帧就绪后原子替换播放器；失败则回滚并保留旧播放器。
pub fn quality_changed(
    ui: &slint::Weak<AppWindow>,
    client: &Arc<crate::network::netease::client::NeteaseClient>,
    rt: &tokio::runtime::Handle,
    idx: i32,
) {
    let idx = idx.clamp(0, 2) as u32;
    let prev = MV_QUALITY.load(Ordering::Relaxed);
    let Some((id, _title, _artist)) = MV_CURRENT.lock().ok().and_then(|g| g.clone()) else {
        MV_QUALITY.store(idx, Ordering::Relaxed);
        persist_quality(idx);
        return;
    };
    // 没有正在播放的实例（理论上不会走到）：只记选择
    if get_player().lock().map(|g| g.is_none()).unwrap_or(true) {
        MV_QUALITY.store(idx, Ordering::Relaxed);
        persist_quality(idx);
        return;
    }
    if prev == idx { return; }

    let label = ["480p", "720p", "1080p"][idx as usize];
    MV_QUALITY.store(idx, Ordering::Relaxed);
    persist_quality(idx);
    let Some(u) = ui.upgrade() else { return };
    let pos_ms = u.get_mv_position().max(0) as u64;
    let playing = u.get_mv_playing();
    // 乐观更新按钮文案 + 顶部胶囊常显切换状态（失败再回滚）
    u.set_mv_quality(idx as i32);
    u.set_mv_switch_tip(format!("正在切换到 {label}").into());
    u.set_mv_switching(true);

    let ui_weak = ui.clone();
    let client = client.clone();
    let rt_ev = rt.clone();
    let token = MV_SWITCH_TOKEN.fetch_add(1, Ordering::Relaxed) + 1;
    rt.spawn(async move {
        let stale = || MV_SWITCH_TOKEN.load(Ordering::Relaxed) != token;
        let fail = |msg: &str| {
            log::warn!(target: "mv", "quality switch to {label} failed: {msg}");
            quality_switch_failed(&ui_weak, prev, "清晰度切换失败，保持当前清晰度");
        };
        // 1. 请求新码率地址（与 play_mv 一致：mv/detail 精确清晰度优先，mv_url 兜底）
        let mut picked: Option<String> = None;
        if let Ok(detail) = client.mv_detail(id as u64).await {
            if stale() { return; }
            picked = pick_detail_url(&detail, idx);
        }
        if picked.is_none() {
            let resp = client.mv_url(id as u64, Some(quality_r(idx))).await;
            if stale() { return; }
            match pick_urls_for_quality(resp, idx).map(|mut v| v.remove(0)) {
                Ok(s) => picked = Some(s),
                Err(e) => { log::error!(target: "mv", "mv_url failed: {e}"); fail("mv_url"); return; }
            }
        }
        let Some(stream_url) = picked else { fail("no stream url"); return; };

        // 2. 后台静音预热新播放器（旧播放器继续出声，画面不中断）
        // 同样保持源分辨率：切到 480p/720p 时画面清晰度应肉眼可辨。
        let cfg = crate::core::mv_config::MvPlaybackConfig::default();
        let mut new_player = MvPlayer::with_config(cfg);
        // 两路订阅互相独立：一路只用于等首帧，一路留给替换后的帧投递
        let wait_rx = new_player.frame_buffer.subscribe();
        let feed_rx = new_player.frame_buffer.subscribe();
        let events_rx = match new_player.start_playback(&stream_url) {
            Ok(rx) => rx,
            Err(e) => { log::error!(target: "mv", "start_playback failed: {e}"); fail("start"); return; }
        };
        new_player.set_volume(0);
        let seek_us = pos_ms * 1000;

        // 3. 先等流首帧：让 first_pts 基准落在流的开头，
        //    否则 seek 后首帧会把基准定在目标位置，进度会被重置成 0。
        let first_ok = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            mv_wait_frame_near(wait_rx.clone(), 0),
        ).await.unwrap_or(false);
        if stale() {
            new_player.stop_playback();
            return;
        }
        if !first_ok {
            new_player.stop_playback();
            fail("first frame timeout");
            return;
        }
        // 4. seek 到当前位置，等新位置出帧
        if seek_us > 0 {
            new_player.seek_to_us(seek_us);
            let seek_ok = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                mv_wait_frame_near(wait_rx, seek_us),
            ).await.unwrap_or(false);
            if stale() {
                new_player.stop_playback();
                return;
            }
            if !seek_ok {
                new_player.stop_playback();
                fail("seek timeout");
                return;
            }
        }
        if stale() {
            new_player.stop_playback();
            return;
        }

        // 5. 原子替换：代数自增（旧订阅失效）→ 停旧播放器 → 装新播放器
        let gen_id = MV_FRAME_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        let new_state = new_player.state();
        new_player.set_volume(MV_VOLUME.load(Ordering::Relaxed));
        if !playing { new_player.toggle_pause(); }
        if let Ok(mut g) = get_player().lock() {
            if let Some(mut old) = g.take() {
                old.stop_playback();
            }
            *g = Some(new_player);
        }
        spawn_frame_feeder(ui_weak.clone(), feed_rx, gen_id);
        spawn_buffered_reporter(ui_weak.clone(), new_state, gen_id);
        spawn_event_forwarder(ui_weak.clone(), events_rx, client, rt_ev, gen_id);

        // 6. 只更新清晰度与提示，不动播放位置/播放器可见性
        let ui_show = ui_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(u) = ui_show.upgrade() {
                u.set_mv_quality(idx as i32);
                u.set_mv_playing(playing);
                u.set_mv_switching(false);
                u.set_mv_hint(format!("已切换到 {label}").into());
            }
        });
    });
}
