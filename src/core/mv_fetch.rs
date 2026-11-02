//! MV 本地加速代理：ffmpeg 连 127.0.0.1 代理，后端多线程 HTTP Range 并行回源。
//!
//! 背景：ffmpeg 协议层对单 URL 只能单连接顺序读，网易云 CDN 单连接限速时
//! 1080p 高码率会"播一下卡一下"。本模块起一个本地 HTTP 代理：
//! - ffmpeg 像访问普通静态文件一样访问代理（支持 Range / seek 重连）；
//! - 代理后端 4 个 worker 线程按 4MB 分片并行预取源站，带宽成倍提升；
//! - 分片缓存带"优先基准"：ffmpeg seek 到文件尾读 moov 时，调度立即跳到该位置；
//! - 已播放位置之前的分片及时释放，内存占用限制在活跃窗口内。
//!
//! 生命周期随 MvPlayer 实例：drop 即关闭监听与全部 worker。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// 分片大小 4MB：1080p 约 6 秒内容；太小调度开销大，太大首片等待久。
const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
/// 并行回源线程数。
const WORKERS: usize = 4;
/// 单片下载超时。
const CHUNK_TIMEOUT: Duration = Duration::from_secs(15);
/// 单片失败重试次数（超过则标记失败，代理断流由上层换源兜底）。
const CHUNK_RETRIES: u32 = 3;
/// 等待分片就绪的超时（源站极慢时避免代理线程永久挂死）。
const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
/// touch 时保留播放位置之前多少片。
/// 策略调整：不再按位置释放已下载分片——整个 MV 下载完成后常驻内存，
/// 任意快进后退都从内存命中，绝不触发重新网络拉取。
/// 单 MV 通常 100~500MB，桌面端可接受；MV 切换时 proxy drop 整体释放。
const KEEP_BEHIND_CHUNKS: u32 = u32::MAX;

#[derive(Clone)]
enum Slot {
    Loading,
    Ready(Arc<Vec<u8>>),
}

struct CacheInner {
    slots: HashMap<u32, Slot>,
    /// 优先调度基准（最近一次请求位置的分片序号）：worker 从这里开始向后找 Pending。
    priority_base: u32,
}

/// 分片缓存 + 下载调度核心（跨 worker / 代理连接线程共享）。
struct ChunkCache {
    url: String,
    size: u64,
    chunk_count: u32,
    inner: Mutex<CacheInner>,
    cond: Condvar,
    shutdown: AtomicBool,
    client: reqwest::blocking::Client,
}

impl ChunkCache {
    fn new(url: &str, size: u64) -> Arc<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(CHUNK_TIMEOUT)
            .build()
            .expect("reqwest blocking client");
        Arc::new(Self {
            url: url.to_string(),
            size,
            chunk_count: size.div_ceil(CHUNK_SIZE) as u32,
            inner: Mutex::new(CacheInner { slots: HashMap::new(), priority_base: 0 }),
            cond: Condvar::new(),
            shutdown: AtomicBool::new(false),
            client,
        })
    }

    fn chunk_range(&self, idx: u32) -> (u64, u64) {
        let start = idx as u64 * CHUNK_SIZE;
        let end = (start + CHUNK_SIZE).min(self.size) - 1;
        (start, end)
    }

    /// 更新优先调度基准（新请求位置）：worker 立即转向该位置向后预取。
    /// 不再释放已下载分片：整个 MV 常驻内存，快进后退均命中缓存。
    /// priority_base 仅用于决定下载优先级（seek 目标优先），不影响最终完整性——
    /// pick_next 会在前方下载完毕后回头补齐 0..priority_base 的分片。
    fn touch(&self, pos: u64) {
        let idx = (pos / CHUNK_SIZE) as u32;
        let mut g = self.inner.lock().unwrap();
        g.priority_base = idx;
        let floor = idx.saturating_sub(KEEP_BEHIND_CHUNKS);
        g.slots.retain(|&i, s| i >= floor || matches!(s, Slot::Loading));
        self.cond.notify_all();
    }

    /// 等待分片就绪。返回数据或 None（失败/超时/shutdown）。
    fn wait_chunk(&self, idx: u32) -> Option<Arc<Vec<u8>>> {
        let deadline = std::time::Instant::now() + WAIT_TIMEOUT;
        let mut g = self.inner.lock().unwrap();
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return None;
            }
            match g.slots.get(&idx) {
                Some(Slot::Ready(d)) => return Some(d.clone()),
                _ => {}
            }
            // 确保该片在调度视野内（ffmpeg 读位置即优先级）
            if idx > g.priority_base {
                // 不提升 priority_base：读顺序天然递增，touch 已负责跳转
            }
            self.cond.notify_all(); // 唤醒 worker 检查是否有 Pending 可领
            let remain = deadline.saturating_duration_since(std::time::Instant::now());
            if remain.is_zero() {
                return None;
            }
            let (guard, _) = self.cond.wait_timeout(g, remain.min(Duration::from_millis(500))).unwrap();
            g = guard;
        }
    }

    /// worker 主循环：领取最接近 priority_base 的 Pending 片下载。
    fn worker_loop(self: &Arc<Self>) {
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            let idx = {
                let mut g = self.inner.lock().unwrap();
                match self.pick_next(&mut g) {
                    Some(i) => {
                        g.slots.insert(i, Slot::Loading);
                        i
                    }
                    None => {
                        let (guard, _) = self
                            .cond
                            .wait_timeout(g, Duration::from_millis(500))
                            .unwrap();
                        drop(guard);
                        continue;
                    }
                }
            };
            match self.download(idx) {
                Ok(data) => {
                    let mut g = self.inner.lock().unwrap();
                    g.slots.insert(idx, Slot::Ready(Arc::new(data)));
                    self.cond.notify_all();
                }
                Err(e) => {
                    log::warn!(target: "mv_fetch", "chunk {idx} download failed: {e}");
                    let mut g = self.inner.lock().unwrap();
                    // 失败分片重新置为未下载（从 slots 移除），让其他 worker 再试。
                    // 连续失败由 download 内部重试计数 + worker_loop 自然重试兜底，
                    // 避免单片永久缺失导致整片永远读不到。
                    g.slots.remove(&idx);
                    self.cond.notify_all();
                }
            }
        }
    }

    /// 领取下一个待下载分片。
    /// 策略：优先下载 priority_base 及之后的分片（播放位置前方，保证流畅播放），
    /// 前方全部就绪后回头补 priority_base 之前的分片（异步缓冲完整文件）。
    /// 这样无论 ffmpeg 首次读取位置在哪（如先读文件尾 moov），最终都会把整个
    /// 文件下载到内存，任意快进后退均命中缓存。
    fn pick_next(&self, g: &mut CacheInner) -> Option<u32> {
        // 1) 播放位置前方：priority_base .. chunk_count
        for idx in g.priority_base..self.chunk_count {
            if !g.slots.contains_key(&idx) {
                return Some(idx);
            }
        }
        // 2) 前方已全部下载/在下载，回头补 0 .. priority_base
        for idx in 0..g.priority_base {
            if !g.slots.contains_key(&idx) {
                return Some(idx);
            }
        }
        None
    }

    /// 下载单个分片（片内自带重试）。
    fn download(&self, idx: u32) -> Result<Vec<u8>, String> {
        let (start, end) = self.chunk_range(idx);
        let mut last_err = String::new();
        for attempt in 0..CHUNK_RETRIES {
            if self.shutdown.load(Ordering::Relaxed) {
                return Err("shutdown".into());
            }
            let resp = self
                .client
                .get(&self.url)
                .header("Range", format!("bytes={start}-{end}"))
                .send();
            match resp {
                Ok(r) if r.status().is_success() || r.status().as_u16() == 206 => {
                    match r.bytes() {
                        Ok(b) => return Ok(b.to_vec()),
                        Err(e) => last_err = format!("body: {e}"),
                    }
                }
                Ok(r) => last_err = format!("status {}", r.status()),
                Err(e) => last_err = format!("{e}"),
            }
            log::debug!(target: "mv_fetch", "chunk {idx} retry {attempt}: {last_err}");
            std::thread::sleep(Duration::from_millis(300 * (attempt as u64 + 1)));
        }
        Err(last_err)
    }
}

/// 本地加速代理句柄。drop 关闭监听与全部 worker。
pub struct MvAccelProxy {
    port: u16,
    cache: Arc<ChunkCache>,
    /// 持有监听 socket（字段本身不读取，Drop 时关闭 accept 循环的 fd）。
    _listener: Arc<TcpListener>,
}

impl MvAccelProxy {
    /// 探测源站大小并启动代理。返回 None 表示不可用（调用方回退直连）。
    pub fn start(url: &str) -> Option<Self> {
        // 1. 探测总大小：GET Range: bytes=0-0 → Content-Range: bytes 0-0/total
        //    （HEAD 在部分 CDN 上不返回长度或被拒，0-0 探测最通用；超时即回退直连）
        //    超时给 6s：CDN 抖动时 3s 误回退直连会失去多线程预取，
        //    直连受包队列背压传导，表现为"播放到缓冲点才重新请求网络"。
        let probe_client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(6))
            .build()
            .ok()?;
        let resp = probe_client.get(url).header("Range", "bytes=0-0").send().ok()?;
        let size = resp
            .headers()
            .get("Content-Range")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.parse::<u64>().ok())
            .or_else(|| resp.headers().get("Content-Length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok()))?;
        if size == 0 {
            return None;
        }

        // 2. 分片缓存 + worker
        let cache = ChunkCache::new(url, size);
        for _ in 0..WORKERS {
            let c = cache.clone();
            std::thread::Builder::new()
                .name("mv-fetch".into())
                .spawn(move || c.worker_loop())
                .ok()?;
        }

        // 3. 本地监听（非阻塞 accept 轮询，便于响应 shutdown）
        let listener = TcpListener::bind("127.0.0.1:0").ok()?;
        listener.set_nonblocking(true).ok()?;
        let port = listener.local_addr().ok()?.port();
        let listener = Arc::new(listener);
        {
            let listener = listener.clone();
            let cache = cache.clone();
            std::thread::Builder::new()
                .name("mv-proxy".into())
                .spawn(move || loop {
                    if cache.shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let c = cache.clone();
                            std::thread::Builder::new()
                                .name("mv-proxy-conn".into())
                                .spawn(move || handle_conn(stream, c))
                                .ok();
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(_) => return,
                    }
                })
                .ok()?;
        }
        log::info!(target: "mv_fetch", "accel proxy started: 127.0.0.1:{port}, size {}MB, chunks {}",
            size / 1048576, cache.chunk_count);
        Some(Self { port, cache, _listener: listener })
    }

    /// ffmpeg 应访问的本地地址。
    pub fn local_url(&self) -> String {
        format!("http://127.0.0.1:{}/video.mp4", self.port)
    }
}

impl Drop for MvAccelProxy {
    fn drop(&mut self) {
        self.cache.shutdown.store(true, Ordering::Relaxed);
        self.cache.cond.notify_all();
        // 触发一次自连，唤醒 accept 轮询之外的潜在阻塞
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        log::info!(target: "mv_fetch", "accel proxy stopped");
    }
}

/// 处理一条 ffmpeg 连接：解析 GET/Range → 从分片缓存流式回写。
fn handle_conn(mut stream: TcpStream, cache: Arc<ChunkCache>) {
    let _ = stream.set_nodelay(true);
    // 1. 读请求头
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return,
            Ok(_) => {
                head.push(byte[0]);
                if head.len() >= 4 && &head[head.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
                if head.len() > 16 * 1024 {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let head = String::from_utf8_lossy(&head);
    if !head.starts_with("GET ") {
        return; // 只支持 GET（ffmpeg 探测/读取均为 GET）
    }
    // 解析 Range: bytes=start-end（end 可省略）
    let mut start: u64 = 0;
    let mut end: u64 = cache.size - 1;
    let mut ranged = false;
    for line in head.lines() {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("Range:").or_else(|| l.strip_prefix("range:")) {
            if let Some(spec) = v.trim().strip_prefix("bytes=") {
                let mut it = spec.split('-');
                if let Some(s) = it.next().and_then(|x| x.parse::<u64>().ok()) {
                    start = s;
                    ranged = true;
                    if let Some(e) = it.next().and_then(|x| x.parse::<u64>().ok()) {
                        end = e.min(cache.size - 1);
                    }
                }
            }
        }
    }
    if start >= cache.size || start > end {
        return;
    }
    cache.touch(start);

    // 2. 写响应头
    let total = end - start + 1;
    let status = if ranged {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\n",
            cache.size
        )
    } else {
        "HTTP/1.1 200 OK\r\n".to_string()
    };
    let resp = format!(
        "{status}Content-Type: video/mp4\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(resp.as_bytes()).is_err() {
        return;
    }

    // 3. 顺序回写 [start, end]：按分片等待就绪 → 直接写 TCP（4MB 整片 write_all，
    //    本地回环带宽足够，ffmpeg 读取速度即消费速度）
    let mut pos = start;
    while pos <= end {
        if cache.shutdown.load(Ordering::Relaxed) {
            return;
        }
        let idx = (pos / CHUNK_SIZE) as u32;
        let Some(data) = cache.wait_chunk(idx) else { return };
        let (cs, ce) = cache.chunk_range(idx);
        let off = (pos - cs) as usize;
        let take = ((ce - pos + 1) as usize).min(data.len() - off).min((end - pos + 1) as usize);
        if stream.write_all(&data[off..off + take]).is_err() {
            return;
        }
        pos += take as u64;
    }
}

// ============================================================================
//  下一首 MV 预缓冲管理器
//  当前 MV 播放时，后台启动下一首的代理（worker 开始下载分片），
//  当前 MV 播完切到下一首时直接复用该代理，实现"秒开 + 已预缓冲"。
// ============================================================================

struct PreBufferedProxy {
    url: String,
    proxy: MvAccelProxy,
}

static NEXT_PROXY: Mutex<Option<PreBufferedProxy>> = Mutex::new(None);

/// 后台预缓冲指定 URL 的 MV（启动代理，worker 开始下载分片）。
/// 若已有预缓冲项，先 drop 旧的（释放 worker 与内存）。
pub fn pre_buffer_next(url: &str) {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return;
    }
    let Some(proxy) = MvAccelProxy::start(url) else { return };
    let mut g = NEXT_PROXY.lock().unwrap();
    *g = Some(PreBufferedProxy { url: url.to_string(), proxy });
    log::info!(target: "mv_fetch", "pre-buffer next MV started: {url}");
}

/// 取出与 url 匹配的预缓冲代理（命中则从全局移除，所有权转交调用方）。
/// 不匹配或无预缓冲时返回 None，并丢弃旧预缓冲（用户手动切歌等场景下
/// 旧预缓冲不再是"下一首"，留着浪费带宽与内存）。
pub fn take_pre_buffered(url: &str) -> Option<MvAccelProxy> {
    let mut g = NEXT_PROXY.lock().unwrap();
    if g.as_ref().map(|p| p.url == url).unwrap_or(false) {
        g.take().map(|p| p.proxy)
    } else {
        *g = None;
        None
    }
}

/// 清除预缓冲（如用户手动切歌、关闭播放器时，避免旧预缓冲残留占用资源）。
pub fn clear_pre_buffer() {
    let mut g = NEXT_PROXY.lock().unwrap();
    *g = None;
}
