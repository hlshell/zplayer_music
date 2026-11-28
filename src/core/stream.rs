//! 流式音频缓冲：下载任务边写、rodio 解码线程边读，实现「首字节到达即出声」。
//!
//! 设计要点：
//! - 缓冲保留全部已下载字节（symphonia 探测/回绕 seek 需要可重复读取）；
//! - 读取端阻塞在 Condvar 上（200ms 超时轮询），下载失败/结束时返回 EOF；
//! - 新播放流创建时会取消上一条流，避免快速连点造成并发下载堆积。
#![allow(dead_code)]

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

struct Buf {
    data: Vec<u8>,
    /// 下载正常结束（已读到 EOF）
    done: bool,
    /// 下载/解码过程中失败
    failed: bool,
    /// 已被新的播放请求取代
    cancelled: bool,
}

/// 下载端与解码端共享的流式缓冲。
pub struct StreamBuffer {
    state: Mutex<Buf>,
    cv: Condvar,
}

/// 最近一条流：新流创建时取消旧流（旧流已被切歌，解码端不会再需要它）。
static LAST: Mutex<Option<Weak<StreamBuffer>>> = Mutex::new(None);

impl StreamBuffer {
    /// 创建缓冲，并取消上一条仍在下载的流。
    pub fn new() -> Arc<Self> {
        let this = Arc::new(Self {
            state: Mutex::new(Buf { data: Vec::new(), done: false, failed: false, cancelled: false }),
            cv: Condvar::new(),
        });
        if let Ok(mut g) = LAST.lock() {
            if let Some(prev) = g.take().and_then(|w| w.upgrade()) {
                prev.cancel();
            }
            *g = Some(Arc::downgrade(&this));
        }
        this
    }

    /// 追加下载数据（已取消的流直接丢弃，交还调用方自行退出）。
    pub fn push(&self, chunk: &[u8]) {
        let mut g = self.state.lock().unwrap();
        if g.cancelled {
            return;
        }
        g.data.extend_from_slice(chunk);
        self.cv.notify_all();
    }

    /// 下载正常结束。
    pub fn finish(&self) {
        let mut g = self.state.lock().unwrap();
        g.done = true;
        self.cv.notify_all();
    }

    /// 下载失败：读取端立刻收到 EOF，播放层据此标记失败。
    pub fn fail(&self) {
        let mut g = self.state.lock().unwrap();
        g.failed = true;
        g.done = true;
        self.cv.notify_all();
    }

    /// 取消（被新播放请求取代）。
    pub fn cancel(&self) {
        let mut g = self.state.lock().unwrap();
        g.cancelled = true;
        g.done = true;
        self.cv.notify_all();
    }

    pub fn failed(&self) -> bool {
        self.state.lock().map(|g| g.failed).unwrap_or(false)
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.lock().map(|g| g.cancelled).unwrap_or(false)
    }

    pub fn is_done(&self) -> bool {
        self.state.lock().map(|g| g.done).unwrap_or(false)
    }

    /// 已缓冲字节数（用于判断首块是否到达）。
    pub fn len(&self) -> usize {
        self.state.lock().map(|g| g.data.len()).unwrap_or(0)
    }

    /// 为 rodio 解码器创建一个独立读取游标。
    pub fn reader(self: &Arc<Self>) -> StreamReader {
        StreamReader { buf: self.clone(), pos: 0 }
    }
}

/// 实现 `Read + Seek` 的解码端游标：数据不足时等待下载线程追加。
pub struct StreamReader {
    buf: Arc<StreamBuffer>,
    pos: u64,
}

impl Read for StreamReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut g = self.buf.state.lock().unwrap();
        loop {
            let len = g.data.len() as u64;
            if self.pos < len {
                let start = self.pos as usize;
                let n = out.len().min(len as usize - start);
                out[..n].copy_from_slice(&g.data[start..start + n]);
                self.pos += n as u64;
                return Ok(n);
            }
            // 无新数据：结束 / 失败 / 取消都视为 EOF，交给解码器收尾
            if g.done {
                return Ok(0);
            }
            let (ng, _) = self
                .buf
                .cv
                .wait_timeout(g, Duration::from_millis(200))
                .unwrap();
            g = ng;
        }
    }
}

impl Seek for StreamReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let mut g = self.buf.state.lock().unwrap();
        match from {
            SeekFrom::Start(n) => {
                self.pos = n;
            }
            SeekFrom::Current(n) => {
                let p = self.pos as i64 + n;
                self.pos = p.max(0) as u64;
            }
            SeekFrom::End(n) => {
                // 末尾位置需要完整长度：等下载结束（或失败/取消）
                while !g.done {
                    let (ng, _) = self
                        .buf
                        .cv
                        .wait_timeout(g, Duration::from_millis(200))
                        .unwrap();
                    g = ng;
                }
                let end = g.data.len() as i64 + n;
                self.pos = end.max(0) as u64;
            }
        }
        Ok(self.pos)
    }
}
