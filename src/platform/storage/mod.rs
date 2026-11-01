//! 存储平台门面：键值 + 二进制 blob 的统一存取。
//! desktop/mobile 用文件系统（FsStorage，base = 应用数据目录），web 用 IndexedDB（后续 #33）。
//! 业务（persist/config 等）只调本门面函数，不直接碰 std::fs 与路径。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

/// 平台存储契约。key 为相对基目录的路径片段（如 "config.toml"、"cache/covers/x.png"）。
/// 字节 / exists / remove 方法为 cache 迁移、web(IndexedDB)、mobile 预留，启用对应能力时移除 allow。
#[allow(dead_code)]
pub trait Storage: Send + Sync {
    fn read_text(&self, key: &str) -> Option<String>;
    fn write_text(&self, key: &str, text: &str) -> std::io::Result<()>;
    fn read_bytes(&self, key: &str) -> Option<Vec<u8>>;
    fn write_bytes(&self, key: &str, bytes: &[u8]) -> std::io::Result<()>;
    fn exists(&self, key: &str) -> bool;
    fn remove(&self, key: &str) -> std::io::Result<()>;
}

#[cfg(feature = "desktop")]
mod fs;
#[cfg(feature = "desktop")]
use fs::FsStorage as DefaultStorage;

#[cfg(feature = "web")]
mod web;
#[cfg(feature = "web")]
use web::WebStorage as DefaultStorage;

// 后续（#33 mobile）：
// #[cfg(feature = "mobile")] mod mobile;    use mobile::MobileStorage as DefaultStorage;

static INSTANCE: OnceLock<Arc<dyn Storage>> = OnceLock::new();

/// 应用启动时初始化（base = 应用数据目录）。
pub fn init(base: PathBuf) {
    let _ = INSTANCE.set(Arc::new(DefaultStorage::new(base)));
}

/// 全局存储实例（未初始化时回退相对 "." 的文件系统存储，保证无 panic）。
fn get() -> &'static dyn Storage {
    INSTANCE
        .get_or_init(|| Arc::new(DefaultStorage::new(PathBuf::from("."))))
        .as_ref()
}

pub fn read_text(key: &str) -> Option<String> {
    get().read_text(key)
}
pub fn write_text(key: &str, text: &str) -> std::io::Result<()> {
    get().write_text(key, text)
}
#[allow(dead_code)] // 预留（cache/web/mobile 迁移时启用）
pub fn read_bytes(key: &str) -> Option<Vec<u8>> {
    get().read_bytes(key)
}
#[allow(dead_code)]
pub fn write_bytes(key: &str, bytes: &[u8]) -> std::io::Result<()> {
    get().write_bytes(key, bytes)
}
#[allow(dead_code)]
pub fn exists(key: &str) -> bool {
    get().exists(key)
}
#[allow(dead_code)]
pub fn remove(key: &str) -> std::io::Result<()> {
    get().remove(key)
}
