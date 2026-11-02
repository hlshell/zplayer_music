//! 文件日志：写入 data_dir/log/app.log，按大小轮转（单文件 1MB，保留 3 份备份）。
//! 通过自定义 Write 包装器实现轮转：WriteLogger 内部保持同一 logger，
//! 累计写入超过 1MB 时自动关闭旧文件、备份后移、开启新文件。

use log::LevelFilter;
use simplelog::{Config as LogConfig, WriteLogger};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use time::OffsetDateTime;

/// 单文件上限（1MB）
const MAX_SIZE: u64 = 1024 * 1024;
/// 备份份数：app.log.1 .. app.log.3
const MAX_BACKUPS: u32 = 3;

/// 当前本地日期 "YYYY-MM-DD"（跨日分隔用；取不到本地偏移时回退 UTC）。
fn now_local_date() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!("{:04}-{:02}-{:02}", now.year(), now.month() as u8, now.day())
}

struct RotatingFile {
    dir: PathBuf,
    file: File,
    size: u64,
    /// 上次写入时的日期；跨 00:00 变化时先插一段日期分隔行（日结式区分，不逐条打印日期）
    last_date: String,
}

impl RotatingFile {
    fn new(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("app.log"))?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self { dir, file, size, last_date: String::new() })
    }

    /// 备份当前 app.log，并开启新文件继续写。
    fn rotate(&mut self) -> io::Result<()> {
        // 丢弃最旧备份
        let _ = std::fs::remove_file(self.dir.join(format!("app.log.{}", MAX_BACKUPS)));
        // 备份依次后移：app.log.2 -> app.log.3、app.log.1 -> app.log.2、app.log -> app.log.1
        for i in (1..=MAX_BACKUPS).rev() {
            let from = if i == 1 {
                self.dir.join("app.log")
            } else {
                self.dir.join(format!("app.log.{}", i - 1))
            };
            let to = self.dir.join(format!("app.log.{}", i));
            let _ = std::fs::rename(from, to);
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("app.log"))?;
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // 日结式日期分隔：跨本地 00:00（或启动首条）时先写一段当天日期
        let today = now_local_date();
        if today != self.last_date {
            let sep = format!("\n========== {} ==========\n", today);
            self.file.write(sep.as_bytes())?;
            self.size += sep.len() as u64;
            self.last_date = today;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        if self.size >= MAX_SIZE {
            let _ = self.rotate();
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// 初始化文件日志（Info 级别）。失败时静默（日志仍可写 stdout）。
/// 同时安装崩溃捕获（panic hook）与异常退出检测。
pub fn init() {
    let dir = crate::persist::config::log_dir();
    match RotatingFile::new(dir) {
        Ok(writer) => {
            let _ = WriteLogger::init(LevelFilter::Info, LogConfig::default(), writer);
        }
        Err(_) => {}
    }
    install_crash_hooks();
}

// ---- 崩溃 / 异常退出 捕获与检测 ----

fn now_str() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", s)
}

/// 正常退出标记文件（窗口关闭保存完成后写入）。启动时缺失 = 上次非正常退出/崩溃。
pub fn mark_clean_exit() {
    let marker = crate::persist::config::data_dir().join("clean_exit.flag");
    let _ = std::fs::write(marker, now_str());
}

fn install_crash_hooks() {
    // 1) 上次退出是否干净：启动时 marker 缺失 → 记录"上次异常退出/崩溃"
    let marker = crate::persist::config::data_dir().join("clean_exit.flag");
    if !marker.exists() {
        log::warn!("[exit] 检测到上次为非正常退出或崩溃（无正常退出标记）");
    }
    // 2) panic hook：记录崩溃位置 + 消息 + 调用栈到 app.log，并落一份 crash.log 便于定位
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload().downcast_ref::<&str>().map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown payload".to_string());
        let loc = info.location().map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
        let bt = std::backtrace::Backtrace::force_capture();
        let line = format!("[panic] at {} | {}", loc, payload);
        log::error!("{}", line);
        let dir = crate::persist::config::log_dir();
        let _ = std::fs::create_dir_all(&dir);
        let full = format!("===== crash @ {} =====\n{}\n{}\n========================\n", now_str(), line, bt);
        let _ = std::fs::write(dir.join("crash.log"), full);
        prev(info);
    }));
}
