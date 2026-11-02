//! 播放器回调：统一播放解析器（resolver）、点击播放、Hero/详情播放全部、
//! 详情页资源拉取、seek、队列清理/移除、收藏、播放/暂停、进度轮询、上一首/下一首/播放模式。
//!
//! resolver 是本模块核心：歌曲缓存优先 → 网络解析下载（写缓存）→ play_at 播放 + 拉歌词；
//! 由 setup 创建并在各播放入口共享。
//!
//! 按职责拆分为子模块：
//! - [`play`]：on_play_song / on_hero_play_all / on_detail_play_all / on_seek /
//!   on_play_toggle / on_prev_song / on_next_song / on_play_mode_changed /
//!   进度轮询 / on_locate_playing / 播放加载动画
//! - [`detail`]：详情页打开（每日精选 / 歌单 / 专辑 / 歌手 / 电台 / MV）/ 搜索过滤 / 返回
//! - [`queue`]：清空队列 / 移除单曲
//! - [`like`]：收藏切换（当前曲 / 指定 id / 详情页资源）

use super::Ctx;
use crate::core::lyrics;
use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod detail;
mod like;
mod play;
mod queue;

/// 供 resolver 入口调用：开始播放加载动画（40ms 定时器按需启停）。
pub(crate) use play::kick_loading_anim;

/// 统一播放解析器签名：uid(真实 i64) / name / artist / dur_sec / offset_sec。
pub type Resolver = Rc<dyn Fn(i64, String, String, f32, f32)>;

/// 构建统一播放解析器（歌曲缓存优先 → 网络解析下载写缓存 → play_at 播放 + 拉歌词）。
/// 注意：Slint int 是 i32，网易云歌曲 ID 可能超过 i32::MAX 溢出为负数，
/// 调用方先把 i32 转回 u32 再按 i64 传入，保证 ID 正确。
pub fn make_resolver(ctx: &Ctx) -> Resolver {
    let client_cb = ctx.client.clone();
    let rt_handle = ctx.rt.clone();
    let ui_weak = ctx.ui.clone();
    let toast_cb = ctx.toast.clone();
    // 连续取音频失败计数：成功清零，失败累加；达上限（5）停止并提示（Send 供 async 捕获）
    let fail_count = Arc::new(AtomicI32::new(0));
    Rc::new(move |uid: i64, name: String, artist: String, dur_sec: f32, offset_sec: f32| {
        let client = client_cb.clone();
        let ui_task = ui_weak.clone();
        let rt_h = rt_handle.clone();
        // 后台预取队列下一首（仅列表循环模式：目标确定；单曲循环重播当前、
        // 随机模式结果不定，均不预取）。使点上一首/下一首时目标歌曲大概率
        // 已在缓存，点击后近乎立即出声。本地歌曲跳过（无需网络预取）。
        if let Some(u) = ui_weak.upgrade() {
            if u.get_play_mode() == 0 {
                if let Some(idx) = crate::core::queue::pick_queue_target(&u, 0, 1) {
                    if let Some(s) = u.get_queue().row_data(idx) {
                        let pid = s.id as u32 as i64;
                        let is_local = !s.url.is_empty() && !s.url.starts_with("http");
                        if pid != uid && !is_local {
                            let client_p = client.clone();
                            rt_h.spawn(async move {
                                let _ = crate::core::fetch::fetch_song_bytes(
                                    &client_p, pid, &s.name, &s.artist, s.duration_sec as u64,
                                ).await;
                            });
                        }
                    }
                }
            }
        }
        // 进入取音频阶段：置播放加载中（封面 loader + 进度条闪烁，Rust 动画驱动）
        if let Some(u) = ui_weak.upgrade() { u.set_playing_loading(true); }
        // 动画定时器按需启停：仅在加载期间以 40ms 推进，结束自停
        kick_loading_anim(&ui_weak);
        // clone 供 async move 捕获（Fn 闭包环境变量不可 move 出）
        let fail_c = fail_count.clone();
        let toast_f = toast_cb.clone();
        rt_h.spawn(async move {
            // 检测是否本地歌曲：current_song.url 非 http(s) 开头即为本地文件路径
            let local_path = ui_task.upgrade()
                .map(|u| u.get_current_song().url.to_string())
                .filter(|u| !u.is_empty() && !u.starts_with("http"));
            let is_local = local_path.is_some();
            // 1) 本地歌曲直读文件；2) 网络歌曲走缓存→解析下载
            let bytes_opt = if let Some(path) = local_path {
                match tokio::task::spawn_blocking(move || std::fs::read(&path)).await {
                    Ok(Ok(b)) => Some(b),
                    Ok(Err(e)) => {
                        log::warn!(target: "local", "read local audio failed: {e}");
                        None
                    }
                    Err(e) => {
                        log::warn!(target: "local", "read local audio join error: {e}");
                        None
                    }
                }
            } else {
                crate::core::fetch::fetch_song_bytes(&client, uid, &name, &artist, dur_sec as u64).await
            };
            if let Some(b) = bytes_opt {
                // 3) 播放（可选续播位置；play_at 会停旧流并解除过渡态）
                crate::platform::audio::play_at(b, dur_sec, offset_sec);
                // 播放开始即同步 playing=true + 解除加载中；连续失败计数清零
                fail_c.store(0, Ordering::Relaxed);
                let ui_set = ui_task.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        let _ = u.set_playing(true);
                        let _ = u.set_playing_loading(false);
                    }
                });
            } else {
                // 获取失败：解除过渡态；按播放模式自动跳下一首兜底（连续失败上限 5 后停止）
                crate::platform::audio::end_switch();
                let n = fail_c.fetch_add(1, Ordering::Relaxed) + 1;
                let ui_set = ui_task.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(u) = ui_set.upgrade() {
                        let _ = u.set_playing(false);
                        let _ = u.set_playing_loading(false);
                        if n >= 5 {
                            toast_f.show("连续多首无法播放，已停止", "warn");
                        } else {
                            toast_f.show("该曲无法播放，已自动跳过", "warn");
                            // 复用已注册的下一首回调（内部按播放模式取目标 + 同一 resolver 播放）
                            let _ = u.invoke_next_song();
                        }
                    }
                });
            }
            // 4) 拉取歌词并填充 UI + 存储时间（供播放进度高亮）
            // 本地歌曲无网络 ID，跳过歌词拉取
            if !is_local {
                if let Ok(v) = crate::network::service::song_lyric(&client, uid).await {
                    if let Some(lrc) = v.get("lrc").and_then(|x| x.get("lyric")).and_then(|x| x.as_str()) {
                        let parsed = lyrics::parse_lrc(lrc);
                        // LYRICS 存一份（进度定位高亮行）；parsed 本体进 invoke 闭包，
                        // 在 UI 线程构造 LyricLine 模型（LyricLine 含 ModelRc 非 Send）
                        *lyrics::LYRICS.lock().unwrap() = parsed.clone();
                        let weak2 = ui_task.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(u) = weak2.upgrade() {
                                u.set_lyrics(Rc::new(VecModel::from(lyrics::build_lyric_lines(&parsed))).into());
                            }
                        });
                    }
                }
            }
        });
    })
}

/// 详情页歌曲封面兜底：响应内歌曲对象全无封面 URL 时（部分接口返回精简形状），
/// 用 song_detail 批量拉完整详情（v3 形状含 al.picUrl）；有封面则原样返回。
pub async fn ensure_covers(client: &crate::network::netease::client::NeteaseClient, songs: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let pre = crate::load::parse_tracks(&songs);
    let non_empty = pre.iter().filter(|d| !d.url.is_empty()).count();
    if pre.is_empty() || non_empty > 0 {
        return songs;
    }
    let ids: Vec<i64> = songs.iter()
        .filter_map(|s| s.get("id").and_then(|i| i.as_i64())).take(500).collect();
    if ids.is_empty() { return songs; }
    if let Ok(dv) = crate::network::service::song_detail(client, &ids).await {
        let full = dv.get("songs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        if !full.is_empty() {
            log::info!("[detail] 封面兜底: song_detail {} 首", full.len());
            return full;
        }
    }
    songs
}

/// 统一入口：注册播放器全部回调。各子模块按职责分发。
pub fn setup(ctx: &Ctx, resolver: &Resolver) {
    play::setup_play(ctx, resolver);
    detail::setup_detail(ctx);
    queue::setup_queue(ctx);
    like::setup_like(ctx);
}
