//! 文件系统存储实现（desktop；mobile 后续可复用沙盒路径）。

use std::io;
use std::path::PathBuf;

use super::Storage;

/// 基于基目录的键值存储：key → base.join(key)。
pub struct FsStorage {
    base: PathBuf,
}

impl FsStorage {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.base.join(key)
    }
}

impl Storage for FsStorage {
    fn read_text(&self, key: &str) -> Option<String> {
        std::fs::read_to_string(self.path(key)).ok()
    }

    fn write_text(&self, key: &str, text: &str) -> io::Result<()> {
        // 原子写：先写临时文件再改名，避免中途崩溃损坏（沿用 config save 语义）。
        let p = self.path(key);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &p)
    }

    fn read_bytes(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(key)).ok()
    }

    fn write_bytes(&self, key: &str, bytes: &[u8]) -> io::Result<()> {
        let p = self.path(key);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, bytes)
    }

    fn exists(&self, key: &str) -> bool {
        self.path(key).exists()
    }

    fn remove(&self, key: &str) -> io::Result<()> {
        std::fs::remove_file(self.path(key))
    }
}
