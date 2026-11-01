//! MV 播放列表 handler（右侧抽屉式，默认隐藏）。
//! 数据源：/mv/exclusive/rcmd（精选）+ /api/mv/all（全部），复用 browse.rs 已缓存的数据。
//! 播放边界：prev/next 在已加载范围内循环，不自动触发分页加载。

use std::sync::{Arc, Mutex};
use slint::{ComponentHandle, Model, ModelRc};

/// 播放列表条目。
#[derive(Clone, Debug)]
pub struct MvQueueItem {
    pub id: i64,
    pub name: String,
    pub artist: String,
    pub cover_url: String,
    /// 显示时长（"mm:ss"；未知为空串）
    pub duration: String,
    /// 封面未加载时的渐变回退（argb_u32）
    pub from: u32,
    pub to: u32,
}

/// MV 播放列表（当前队列 + 高亮索引）。
pub struct MvQueue {
    items: Vec<MvQueueItem>,
    current: usize,
}

impl MvQueue {
    pub fn new() -> Self {
        Self { items: Vec::new(), current: 0 }
    }
    pub fn set_items(&mut self, items: Vec<MvQueueItem>, current: usize) {
        self.items = items;
        self.current = current.min(self.items.len().saturating_sub(1));
    }
    /// 当前队列长度（失败跳转时用于计算上限）。
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// 当前播放索引（抽屉打开时定位滚动用）。
    pub fn current_index(&self) -> usize {
        self.current
    }
    pub fn get_current(&self) -> Option<&MvQueueItem> {
        self.items.get(self.current)
    }
    /// 按真实 MV id 定位当前索引；返回是否命中。
    pub fn set_current_by_id(&mut self, id: i64) -> bool {
        if let Some(i) = self.items.iter().position(|x| x.id == id) {
            self.current = i;
            true
        } else {
            false
        }
    }
    /// 播放上一首；在已加载范围内循环（首条→末条）。
    pub fn prev(&mut self) -> Option<&MvQueueItem> {
        if self.items.is_empty() { return None; }
        self.current = if self.current == 0 { self.items.len() - 1 } else { self.current - 1 };
        self.items.get(self.current)
    }
    /// 预告下一首（不改变 current）：失败自动跳转前，用于告知用户将播放哪一首。
    pub fn peek_next(&self) -> Option<&MvQueueItem> {
        if self.items.is_empty() { return None; }
        self.items.get((self.current + 1) % self.items.len())
    }

    /// 播放下一首；在已加载范围内循环（末条→首条）。
    /// 仅在已加载的数据里循环，不自动触发分页加载。
    pub fn next(&mut self) -> Option<&MvQueueItem> {
        if self.items.is_empty() { return None; }
        self.current = (self.current + 1) % self.items.len();
        self.items.get(self.current)
    }
    /// 把条目插入到当前播放位置的下一位并设为当前（“下一首播放”语义）。
    /// 保留原队列：不替换、不清空——搜索/浏览页点 MV 都应插进正在播放的队列。
    /// 已存在于队列时直接定位到该条目（去重，避免同一 MV 反复插入）。
    /// 返回定位后的索引。
    pub fn insert_next(&mut self, item: MvQueueItem) -> usize {
        if let Some(i) = self.items.iter().position(|x| x.id == item.id) {
            self.current = i;
            return i;
        }
        let pos = if self.items.is_empty() { 0 } else { (self.current + 1).min(self.items.len()) };
        self.items.insert(pos, item);
        self.current = pos;
        pos
    }
    /// 播放指定条目（抽屉点选）：按真实 MV id 定位 current 并返回条目。
    /// 注：不用索引——Slint for 循环回调闭包捕获的 index 在 ScrollView 虚拟化下
    /// 读到的是共享槽终值（实测恒为最后可见行），故改用数据自带的 id。
    pub fn play_id(&mut self, id: i64) -> Option<&MvQueueItem> {
        let i = self.items.iter().position(|x| x.id == id)?;
        self.current = i;
        self.items.get(self.current)
    }
}

pub static MV_QUEUE: std::sync::OnceLock<Arc<Mutex<MvQueue>>> = std::sync::OnceLock::new();
pub fn queue() -> &'static Arc<Mutex<MvQueue>> {
    MV_QUEUE.get_or_init(|| Arc::new(Mutex::new(MvQueue::new())))
}

/// 把队列快照同步到 UI（抽屉数据源）。open/切歌/分页加载后调用。
/// 封面先留空，后台用 fetch_covers 异步回填（复用 browse.rs 磁盘缓存，命中即出）。
pub fn sync_ui(u: &crate::AppWindow) {
    let (entries, cover_urls, current) = {
        let Some(q) = queue().lock().ok() else { return };
        let urls: Vec<String> = q.items.iter().map(|it| it.cover_url.clone()).collect();
        (q.snapshot_entries(), urls, q.current_index() as i32)
    };
    let model_rc: ModelRc<crate::MvQueueEntry> = std::rc::Rc::new(slint::VecModel::from(entries)).into();
    u.set_mv_queue(model_rc);
    // 同步当前播放索引：抽屉打开时自动滚动定位到正在播放的卡片
    u.set_mv_queue_current(current);

    // 后台异步加载封面（复用磁盘缓存）
    let ui_weak = ComponentHandle::as_weak(u);
    spawn_fill_covers(cover_urls, ui_weak);
}

/// 后台异步加载封面 → 回填 ModelRc。
/// 先 fetch_covers（磁盘缓存命中立即返回），再用 set_row_data 逐行更新。
fn spawn_fill_covers(
    cover_urls: Vec<String>,
    ui: slint::Weak<crate::AppWindow>,
) {
    let page_urls: Vec<(String, crate::persist::cache::CoverType)> = cover_urls.iter()
        .filter(|u| !u.is_empty())
        .map(|u| (u.clone(), crate::persist::cache::CoverType::Mv))
        .collect();
    if page_urls.is_empty() { return; }

    // 本函数在 UI 线程执行：Handle::try_current() 取不到 runtime，必须用启动时注入的全局句柄
    let Some(rt) = crate::load::home::covers::runtime() else {
        log::warn!(target: "mv", "runtime handle not injected, covers skipped");
        return;
    };

    rt.spawn(async move {
        let covers = crate::load::fetch_covers(&page_urls).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(u) = ui.upgrade() else { return };
            let model = u.get_mv_queue();
            for (idx, url) in cover_urls.iter().enumerate() {
                if url.is_empty() { continue; }
                let Some(buf) = covers.get(url.as_str()).and_then(|x| x.as_ref()) else { continue };
                let Some(mut row) = model.row_data(idx) else { continue };
                row.cover = slint::Image::from_rgba8(buf.clone());
                model.set_row_data(idx, row);
            }
        });
    });
}

impl MvQueue {
    /// 生成 UI 条目（playing 标记 current；封面先留空，后台异步填充）。
    fn snapshot_entries(&self) -> Vec<crate::MvQueueEntry> {
        self.items.iter().enumerate().map(|(i, it)| {
            crate::MvQueueEntry {
                id: it.id as i32,
                name: it.name.clone().into(),
                artist: it.artist.clone().into(),
                duration: it.duration.clone().into(),
                playing: i == self.current,
                cover: slint::Image::default(),
                from: color_from_u32(it.from),
                to: color_from_u32(it.to),
            }
        }).collect()
    }
}

fn color_from_u32(v: u32) -> slint::Color {
    let a = ((v >> 24) & 0xff) as u8;
    let r = ((v >> 16) & 0xff) as u8;
    let g = ((v >> 8) & 0xff) as u8;
    let b = (v & 0xff) as u8;
    slint::Color::from_argb_u8(a, r, g, b)
}
