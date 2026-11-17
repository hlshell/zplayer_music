//! 排行榜页（nav=6）：首次切入时拉取官方榜单列表，填充榜单名 / 封面 / 更新频率 / 前三首预览。
//! 点击榜单由 UI 直接走 `open-resource-detail(0, id)`（网易云榜单详情即歌单）。

use super::Ctx;
use crate::core::song_util::{fmt_plays_cat, from, to};
use crate::persist::cache::CoverType;
use slint::{Model, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 请求进行中标记：切页来回时避免重复发起请求
static LOADING: AtomicBool = AtomicBool::new(false);

/// 解析后的榜单行（与 UI 解耦，便于先填列表再异步补封面）
struct Row {
    id: i64,
    name: String,
    cover_url: String,
    update_freq: String,
    meta: String,
    tops: [String; 3],
}

impl Row {
    fn to_ui(&self, idx: usize) -> crate::Toplist {
        crate::Toplist {
            id: crate::core::ids::remember(0, self.id),
            name: self.name.clone().into(),
            cover: crate::load::cover_now(&self.cover_url),
            update_freq: self.update_freq.clone().into(),
            meta: self.meta.clone().into(),
            top1: self.tops[0].clone().into(),
            top2: self.tops[1].clone().into(),
            top3: self.tops[2].clone().into(),
            from: from(idx),
            to: to(idx),
        }
    }
}

/// 解析 `/weapi/toplist` 响应。
/// 兼容两种包裹：`{"list": [...]}`（经典）与 `{"data": [...]}`（首页热榜取数用的形态）。
fn parse_list(v: &serde_json::Value) -> Vec<Row> {
    let arr = v
        .get("list")
        .and_then(|a| a.as_array())
        .or_else(|| v.get("data").and_then(|a| a.as_array()))
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for it in &arr {
        let name = it
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        // 榜单 id 超过 i32（Slint int）：登记后交给 UI，点击时再还原真实 id
        let Some(id) = it.get("id").and_then(|x| x.as_i64()) else { continue };
        if name.is_empty() {
            continue;
        }
        let cover_url = it.get("coverImgUrl").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let update_freq = it.get("updateFrequency").and_then(|x| x.as_str()).unwrap_or("").to_string();
        // 「100首 · 1234.5万播放」：字段缺失时自动省略，避免出现孤立的「·」
        let track_count = it.get("trackCount").and_then(|x| x.as_u64()).unwrap_or(0);
        let play_count = it.get("playCount").and_then(|x| x.as_u64()).unwrap_or(0);
        let mut parts: Vec<String> = Vec::new();
        if track_count > 0 {
            parts.push(format!("{track_count}{}", crate::i18n::t("toplist-tracks-suffix")));
        }
        if play_count > 0 {
            parts.push(format!("{}{}", fmt_plays_cat(play_count), crate::i18n::t("toplist-plays-suffix")));
        }
        let mut tops: [String; 3] = std::array::from_fn(|_| String::new());
        if let Some(tracks) = it.get("tracks").and_then(|x| x.as_array()) {
            let mut k = 0;
            for t in tracks {
                if k >= 3 {
                    break;
                }
                let (song, artist) = track_names(t);
                if song.is_empty() {
                    continue;
                }
                tops[k] = if artist.is_empty() {
                    format!("{}. {}", k + 1, song)
                } else {
                    format!("{}. {} - {}", k + 1, song, artist)
                };
                k += 1;
            }
        }
        out.push(Row { id, name, cover_url, update_freq, meta: parts.join(" · "), tops });
    }
    out
}

/// 榜单 `tracks` 元素取「歌名 + 歌手」：兼容 `{first,second}`、`{name,ar[]}`、纯字符串三种形态
fn track_names(t: &serde_json::Value) -> (String, String) {
    if let Some(s) = t.as_str() {
        return (s.to_string(), String::new());
    }
    let song = t
        .get("first")
        .or_else(|| t.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let artist = t
        .get("second")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            t.get("ar")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .and_then(|x| x.get("name"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_default();
    (song, artist)
}

pub fn setup(ctx: &Ctx) {
    let ui = ctx.ui.clone();
    let client = ctx.client.clone();
    let rt = ctx.rt.clone();

    super::on_page_fill(ctx, move |nav| {
        if nav != 6 {
            return;
        }
        let Some(u) = ui.upgrade() else { return };
        if u.get_toplists_loaded() {
            return;
        }
        if LOADING.swap(true, Ordering::Relaxed) {
            return;
        }
        u.set_toplists_loading(true);

        let ui_task = ui.clone();
        let client_task = client.clone();
        let rt_covers = rt.clone();
        rt.spawn(async move {
            let parsed = match crate::network::service::toplist(&client_task).await {
                Ok(v) => parse_list(&v),
                Err(e) => {
                    log::info!("[toplist] 榜单列表拉取失败: {e}");
                    Vec::new()
                }
            };
            let ok = !parsed.is_empty();
            let parsed = Arc::new(parsed);

            let _ = slint::invoke_from_event_loop(move || {
                LOADING.store(false, Ordering::Relaxed);
                let Some(u) = ui_task.upgrade() else { return };
                u.set_toplists_loading(false);
                // 失败时不置 loaded：下次切回排行榜可重新尝试
                if !ok {
                    return;
                }
                let items: Vec<crate::Toplist> = parsed.iter().enumerate().map(|(i, r)| r.to_ui(i)).collect();
                u.set_toplists(Rc::new(VecModel::from(items)).into());
                u.set_toplists_loaded(true);

                // 封面：先落列表（有磁盘缓存则立即显示），未命中的后台补图
                let url_idx: Vec<(usize, String)> = parsed
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.cover_url.is_empty())
                    .map(|(i, r)| (i, r.cover_url.clone()))
                    .collect();
                if url_idx.is_empty() {
                    return;
                }
                let urls: Vec<(String, CoverType)> = url_idx
                    .iter()
                    .map(|(_, u)| (u.clone(), CoverType::Playlist))
                    .collect();
                let ui_covers = ui_task.clone();
                let rt_covers = rt_covers.clone();
                rt_covers.spawn(async move {
                    let covers = crate::load::fetch_covers(&urls).await;
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(u) = ui_covers.upgrade() else { return };
                        let model = u.get_toplists();
                        for (i, url) in &url_idx {
                            let Some(buf) = covers.get(url).and_then(|c| c.as_ref()) else { continue };
                            if let Some(mut row) = model.row_data(*i) {
                                row.cover = slint::Image::from_rgba8(buf.clone());
                                model.set_row_data(*i, row);
                            }
                        }
                    });
                });
            });
        });
    });
}
