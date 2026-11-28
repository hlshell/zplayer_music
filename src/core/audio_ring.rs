//! 单生产者/单消费者无锁环形缓冲（f32 交错 PCM）。
//!
//! 用例：解码线程（生产者）写入，实时音频回调（消费者）读取。
//! 实时回调只做内存搬移，不加锁、不分配、不解码——这是系统被外部进程
//! 占满 CPU 时仍能不断音的前提（回调里任何锁都可能被低优先级线程卡住）。
//!
//! 约定：
//! - 写入端只能有一个线程调用 `push`；
//! - 读取端只能有一个线程调用 `pop` / `discard`；
//! - 容量自动向上取 2 的幂，用位与替代取模。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct SpscRing {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    /// 生产者写游标（单调递增，不取模）
    head: AtomicUsize,
    /// 消费者读游标（单调递增，不取模）
    tail: AtomicUsize,
}

// 安全性：head 只由生产者推进、tail 只由消费者推进；
// 同一下标同一时刻只会被一方访问（生产者写 head..head+free，消费者读 tail..head）。
unsafe impl Sync for SpscRing {}
unsafe impl Send for SpscRing {}

impl SpscRing {
    /// 构造容量不小于 `min_samples` 的环（实际容量取 2 的幂）。
    pub fn with_capacity(min_samples: usize) -> Self {
        let cap = min_samples.max(2).next_power_of_two();
        let buf = (0..cap)
            .map(|_| UnsafeCell::new(0.0f32))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            buf,
            mask: cap - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// 已填充（未消费）的样本数。任意线程可调用。
    pub fn len(&self) -> usize {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    /// 单生产者调用：写入一段样本，返回实际写入数量（环满则截断）。
    pub fn push(&self, data: &[f32]) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let free = self.capacity() - head.wrapping_sub(tail);
        let n = free.min(data.len());
        for (i, &v) in data[..n].iter().enumerate() {
            // 安全性：该下标的写权限仅属于生产者（tail 之后、head 之前未发布）
            unsafe { self.buf[(head + i) & self.mask].get().write(v) };
        }
        // Release：保证样本数据先于 head 对消费者可见
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    /// 单消费者调用：读取一段样本，返回实际读出数量（不足则截断并补 0）。
    pub fn pop(&self, out: &mut [f32]) -> usize {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        let n = head.wrapping_sub(tail).min(out.len());
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            // 安全性：该下标的读权限仅属于消费者（head 之前已发布）
            *slot = unsafe { self.buf[(tail + i) & self.mask].get().read() };
        }
        // Release：保证读完成后 tail 才推进，生产者不会覆盖未读完的数据
        self.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
    }

    /// 单消费者调用：丢弃全部未读数据（切歌/seek 清环，不搬移内存）。
    pub fn discard(&self) {
        let head = self.head.load(Ordering::Acquire);
        self.tail.store(head, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::SpscRing;
    use std::sync::Arc;

    #[test]
    fn basic_push_pop() {
        let r = SpscRing::with_capacity(8);
        assert_eq!(r.capacity(), 8);
        let data: Vec<f32> = (0..5).map(|i| i as f32).collect();
        assert_eq!(r.push(&data), 5);
        assert_eq!(r.len(), 5);
        let mut out = [0f32; 8];
        assert_eq!(r.pop(&mut out), 5);
        assert_eq!(&out[..5], &data[..]);
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn truncates_when_full() {
        let r = SpscRing::with_capacity(4);
        let data = [1f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        assert_eq!(r.push(&data), 4);
        assert_eq!(r.push(&data), 0);
        let mut out = [0f32; 2];
        assert_eq!(r.pop(&mut out), 2);
        assert_eq!(out, [1.0, 2.0]);
    }

    #[test]
    fn wraps_around_and_discard() {
        let r = SpscRing::with_capacity(4);
        let mut out = [0f32; 3];
        assert_eq!(r.push(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(r.pop(&mut out), 3);
        // 游标越过容量边界后仍能正确回绕
        assert_eq!(r.push(&[4.0, 5.0, 6.0]), 3);
        assert_eq!(r.pop(&mut out), 3);
        assert_eq!(out, [4.0, 5.0, 6.0]);

        r.discard();
        assert_eq!(r.len(), 0);
        assert_eq!(r.push(&[7.0, 8.0]), 2);
        assert_eq!(r.pop(&mut out), 2);
        assert_eq!(out[..2], [7.0, 8.0]);
    }

    #[test]
    fn cross_thread_order_preserved() {
        let ring = Arc::new(SpscRing::with_capacity(256));
        let producer_ring = ring.clone();
        let producer = std::thread::spawn(move || {
            let mut next = 0f32;
            let mut batch = [0f32; 7];
            for _ in 0..500 {
                for v in batch.iter_mut() {
                    *v = next;
                    next += 1.0;
                }
                let mut off = 0;
                while off < batch.len() {
                    let n = producer_ring.push(&batch[off..]);
                    off += n;
                    if n == 0 {
                        std::thread::sleep(std::time::Duration::from_micros(50));
                    }
                }
            }
            next as usize
        });

        let mut out = [0f32; 11];
        let mut expect = 0usize;
        let mut got = 0usize;
        while got < 3500 {
            let n = ring.pop(&mut out);
            for &v in out.iter().take(n) {
                assert_eq!(v, expect as f32);
                expect += 1;
            }
            got += n;
            if n == 0 {
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        }
        assert_eq!(producer.join().unwrap(), 3500);
    }
}
