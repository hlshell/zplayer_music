//! 封面管道：磁盘缓存优先 + 网络并行下载 + 解码为 RGBA 像素缓冲。
//! 解码/像素缓冲为 Send，在后台线程完成；UI 线程仅用 cover_image 构造 slint::Image。

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use crate::persist::cache::CoverType;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

type Buf = SharedPixelBuffer<Rgba8Pixel>;

/// 全局共享 HTTP Client：封面批量填充非常频繁（首页多区块 / 详情页 / 搜索并发），
/// 每次 fetch 新建 Client 会重建 TLS 配置与连接池，属于纯浪费。
fn http() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| crate::network::proxy::build_client(Some(Duration::from_secs(8))))
}

/// 在途请求表：同一 URL 的并发请求只真正发起一次，后来者排队等同一份结果。
type Waiter = tokio::sync::oneshot::Sender<Option<Buf>>;

fn inflight() -> &'static Mutex<HashMap<String, Vec<Waiter>>> {
    static S: OnceLock<Mutex<HashMap<String, Vec<Waiter>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// owner 路径异常退出（panic / 提前 return）时兜底清空等待者：
/// Sender 被 drop 会让 receiver 收到 Err，等待者转 None 而不是永久挂起。
struct DrainGuard(String);

impl Drop for DrainGuard {
    fn drop(&mut self) {
        inflight().lock().unwrap().remove(&self.0);
    }
}

/// 单 URL 封面获取：dcache → 磁盘 → 网络，同 URL 并发合并。
/// 返回像素缓冲是 Send 的，可在任意线程调用；UI 线程用 cover_now / cover_warm。
pub async fn cover_buffer(url: &str, t: CoverType) -> Option<Buf> {
    if url.is_empty() {
        return None;
    }
    if let Some(hit) = dcache::get(url) {
        return Some(hit);
    }

    // 已在途：排队等 owner 的结果
    let rx = {
        let mut m = inflight().lock().unwrap();
        match m.get_mut(url) {
            Some(waiters) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                waiters.push(tx);
                Some(rx)
            }
            None => {
                m.insert(url.to_string(), Vec::new());
                None
            }
        }
    };
    if let Some(rx) = rx {
        return rx.await.ok().flatten();
    }

    // owner：任何异常退出都由 guard 兜底清空等待者，避免永久挂起
    let _guard = DrainGuard(url.to_string());
    let result = fetch_from_disk_or_net(url, t).await;
    let waiters = inflight().lock().unwrap().remove(url).unwrap_or_default();
    for w in waiters {
        let _ = w.send(result.clone());
    }
    result
}

/// dcache → 磁盘缓存 → 网络下载，未命中磁盘时下载并写盘。
async fn fetch_from_disk_or_net(url: &str, t: CoverType) -> Option<Buf> {
    // 磁盘缓存命中：直接读字节解码，跳过网络
    if let Some(bytes) = crate::persist::cache::load_cover(url, t) {
        let buf = tokio::task::spawn_blocking(move || decode(&bytes))
            .await
            .ok()
            .flatten();
        if let Some(b) = &buf {
            dcache::put(url, b);
        }
        return buf;
    }
    let Ok(resp) = http().get(url).send().await else {
        return None;
    };
    let Ok(bytes) = resp.bytes().await else {
        return None;
    };
    let save_url = url.to_string();
    // 解码是封面显示的必需步骤，必须在此等结果；写盘（WebP 编码 + 落盘）不阻塞显示，
    // 交给独立后台任务 fire-and-forget，避免封面批量下载时写盘拖慢返回。
    let buf = tokio::task::spawn_blocking(move || decode(&bytes))
        .await
        .ok()
        .flatten();
    if let Some(b) = &buf {
        dcache::put(url, b);
        // clone 共享底层像素（引用计数），写盘在后台完成
        let save_buf = b.clone();
        let u = save_url.clone();
        let _ = tokio::task::spawn_blocking(move || save_cover_scaled(&u, t, &save_buf));
    }
    buf
}

/// 解码图像字节为 RGBA 像素缓冲。
/// 统一缩到 600px 内（等比 thumbnail）：网易云原图可达 1000~3000px，
/// 全尺寸解码会让首页几十张封面占用数 GB 内存 + GPU 纹理上传巨大（卡顿根源）。
fn decode(buf: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    image::load_from_memory(buf).ok().and_then(|img| {
        let rgba = img.thumbnail(600, 600).to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        Some(SharedPixelBuffer::clone_from_slice(rgba.as_raw(), w, h))
    })
}

/// 进程内解码缓存：URL → 已解码 600px 像素缓冲。
/// 磁盘缓存存的是 PNG 压缩字节，每次读盘都要解码（600px 也要几十 ms，debug 构建更大）；
/// 同一会话里首页/播放栏/抽屉/详情页会反复取同一张封面，页面切换重建模型还会丢掉旧 Image。
/// 按 256MB 字节预算 FIFO 驱逐（600×600 一张约 1.44MB，约容纳 170 张整页封面），
/// 命中后磁盘 I/O 与解码全省。
mod dcache {
    use super::{Rgba8Pixel, SharedPixelBuffer};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Mutex, OnceLock};

    const BUDGET: u64 = 256 * 1024 * 1024;

    struct Cache {
        map: HashMap<String, SharedPixelBuffer<Rgba8Pixel>>,
        order: VecDeque<String>,
        bytes: u64,
    }

    fn store() -> &'static Mutex<Cache> {
        static S: OnceLock<Mutex<Cache>> = OnceLock::new();
        S.get_or_init(|| Mutex::new(Cache {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }))
    }

    fn size(buf: &SharedPixelBuffer<Rgba8Pixel>) -> u64 {
        buf.width() as u64 * buf.height() as u64 * 4
    }

    /// 命中返回克隆（引用计数共享底层像素，不会深拷贝整块数据）。
    pub fn get(url: &str) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        store().lock().unwrap().map.get(url).cloned()
    }

    /// 未命中才写入；超预算时从最旧条目开始驱逐。
    pub fn put(url: &str, buf: &SharedPixelBuffer<Rgba8Pixel>) {
        if url.is_empty() {
            return;
        }
        let mut c = store().lock().unwrap();
        if c.map.contains_key(url) {
            return;
        }
        c.bytes += size(buf);
        c.order.push_back(url.to_string());
        c.map.insert(url.to_string(), buf.clone());
        while c.bytes > BUDGET {
            let Some(old) = c.order.pop_front() else { break };
            if let Some(b) = c.map.remove(&old) {
                c.bytes = c.bytes.saturating_sub(size(&b));
            }
        }
    }
}

/// 并行获取一组封面（磁盘缓存优先，未命中下载并写盘），解码为 RGBA 像素缓冲（去重）。
/// 失败/无 URL 的条目为 None。urls 携带类型，缓存按类型分子目录存取。
pub async fn fetch_covers(urls: &[(String, CoverType)]) -> HashMap<String, Option<Buf>> {
    let mut seen = HashSet::new();
    let unique: Vec<(String, CoverType)> = urls
        .iter()
        .filter(|(u, _)| !u.is_empty() && seen.insert(u.clone()))
        .cloned()
        .collect();
    if unique.is_empty() {
        return HashMap::new();
    }
    // 并发闸：受"下载线程数"设置（threads-index）控制，节流并发网络下载。
    // 0=1 1=2 2=4 3=8 else=16；dcache 命中不受闸约束（微秒级，立即返回）。
    let threads = {
        let cfg = crate::persist::config::AppConfig::load();
        match cfg.network.threads_index {
            0 => 1usize, 1 => 2, 2 => 4, 3 => 8, _ => 16,
        }
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(threads));
    let handles: Vec<_> = unique
        .iter()
        .map(|(u, t)| {
            let u = u.clone();
            let t = *t;
            let sem = sem.clone();
            tokio::spawn(async move {
                let _permit = sem.acquire().await.ok();
                cover_buffer(&u, t).await
            })
        })
        .collect();
    let mut map = HashMap::new();
    for ((u, _), h) in unique.iter().zip(handles) {
        map.insert(u.clone(), h.await.ok().flatten());
    }
    map
}

// ==================== UI 线程门面 ====================
// 所有界面取封面统一走这两个入口：
//   cover_now  同步拿当前已有的图（只查内存解码缓存，绝不读盘/解码）
//   cover_warm 未命中时排队后台补齐，完成后在 UI 线程回调
// 这样 UI 线程上不存在任何磁盘解码路径，也就不会再有"某页同步解码卡顿"的事故。

/// tokio 运行时句柄（main 启动时注入），供 cover_warm 在 UI 线程发起后台任务。
static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();

pub fn set_runtime(h: tokio::runtime::Handle) {
    let _ = RT.set(h);
}

/// 取全局运行时句柄：UI 线程里 Handle::try_current() 取不到，
/// 需要发起后台异步任务的调用点统一走这里（未注入时返回 None）。
pub fn runtime() -> Option<tokio::runtime::Handle> {
    RT.get().cloned()
}

thread_local! {
    /// 待回填回调（仅 UI 线程访问）：同 URL 的多次请求合并成一次后台获取。
    static PENDING: std::cell::RefCell<HashMap<String, Vec<Box<dyn FnOnce(Image)>>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// UI 线程同步取图：只查进程内解码缓存，未命中返回空图（由调用点自行决定是否 cover_warm）。
pub fn cover_now(url: &str) -> Image {
    dcache::get(url).map(Image::from_rgba8).unwrap_or_default()
}

/// UI 线程：命中缓存立即回调；未命中则注册回调并后台获取，完成后回 UI 线程派发。
/// 同一 URL 并发多次调用只发起一次获取（PENDING 表合并）。
/// 回调里应自行校验窗口存活与数据是否仍然相关（例如 current_song.id 未变）。
pub fn cover_warm(url: &str, t: CoverType, on_ready: impl FnOnce(Image) + 'static) {
    if url.is_empty() {
        return;
    }
    if let Some(buf) = dcache::get(url) {
        on_ready(Image::from_rgba8(buf));
        return;
    }
    // 注册回调；first=true 表示本线程是首个注册者，负责发起后台获取
    let first = PENDING.with(|p| {
        let mut m = p.borrow_mut();
        let cbs = m.entry(url.to_string()).or_default();
        cbs.push(Box::new(on_ready));
        cbs.len() == 1
    });
    if !first {
        return;
    }
    let Some(h) = RT.get() else {
        // 运行时未注入（理论上不会发生）：立即派发空图，避免回调永久挂起
        let cbs = PENDING.with(|p| p.borrow_mut().remove(url).unwrap_or_default());
        for cb in cbs {
            cb(Image::default());
        }
        return;
    };
    let u = url.to_string();
    h.spawn(async move {
        let buf = cover_buffer(&u, t).await;
        // 像素缓冲是 Send 的，跨线程只传它；slint::Image 只能回 UI 线程构造
        let _ = slint::invoke_from_event_loop(move || {
            let cbs = PENDING.with(|p| p.borrow_mut().remove(&u).unwrap_or_default());
            let img = buf.map(Image::from_rgba8).unwrap_or_default();
            for cb in cbs {
                cb(img.clone());
            }
        });
    });
}

/// UI 线程批量补图：rows 为（行索引, URL, 类型），未命中内存缓存的行自动后台补齐，
/// 每张就绪时以（行索引, 图）回调；同 URL 只获取一次。
/// 列表/网格统一用法：模型先用 cover_now 建占位 → set 模型 → 调这里逐行回填。
pub fn warm_rows(
    rows: Vec<(usize, String, CoverType)>,
    on_ready: std::rc::Rc<dyn Fn(usize, Image)>,
) {
    for (idx, url, t) in rows {
        if url.is_empty() {
            continue;
        }
        let cb = on_ready.clone();
        cover_warm(&url, t, move |img| cb(idx, img));
    }
}

/// 下载的封面统一缩到 600px 内再写盘：网易云原图可达数 MB（1000~3000px），
/// 直接存原图会让磁盘命中后的解码仍耗时 1.5s+（启动恢复/首页首批卡顿根源）；
/// 缩放后解码 <100ms，且缩小缓存体积。
/// 入参已是 decode() 产出的 600px RGBA 缓冲，直接编码写盘，不重复解码原图。
///
/// 磁盘格式：优先 WebP 有损（照片/插画封面压缩率远优于 PNG，2000 张可省约一半配额）；
/// 若缓冲含透明像素则回退 PNG（无损）以保留 alpha，避免透明封面被压黑底。
fn save_cover_scaled(url: &str, t: CoverType, buf: &SharedPixelBuffer<Rgba8Pixel>) {
    let Some(img) = image::RgbaImage::from_raw(buf.width(), buf.height(), buf.as_bytes().to_vec())
    else { return; };
    let has_alpha = img.pixels().any(|p| p[3] != 255);
    let fmt = if has_alpha { image::ImageFormat::Png } else { image::ImageFormat::WebP };
    let mut out = std::io::Cursor::new(Vec::new());
    if image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, fmt)
        .is_ok()
    {
        crate::persist::cache::save_cover(url, t, out.get_ref());
    }
}

/// UI 线程专用：把解码后的像素缓冲转成 slint::Image（空则回退渐变）
fn cover_image(b: Option<&SharedPixelBuffer<Rgba8Pixel>>) -> Image {
    match b {
        Some(buf) => Image::from_rgba8(buf.clone()),
        None => Image::default(),
    }
}

/// 按 URL 从封面集取封面图
pub(crate) fn cov(covers: &HashMap<String, Option<SharedPixelBuffer<Rgba8Pixel>>>, url: &str) -> Image {
    cover_image(covers.get(url).and_then(|x| x.as_ref()))
}
