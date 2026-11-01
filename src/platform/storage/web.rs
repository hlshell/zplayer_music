//! Web 存储后端（阶段 1a 最小实现）：localStorage + base64 编码二进制。
//! 满足 Storage 同步契约；后续（阶段 1b）可换 IndexedDB（异步，需适配契约）。
use super::Storage;
use std::path::PathBuf;

use base64::Engine as _;
use wasm_bindgen::JsCast;
use web_sys::window;

pub struct WebStorage {
    prefix: String,
}

impl WebStorage {
    pub fn new(_base: PathBuf) -> Self {
        WebStorage { prefix: "zplayer".into() }
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}/{}", self.prefix, key)
    }

    fn storage() -> Option<web_sys::Storage> {
        window()?.local_storage().ok().flatten()
    }
}

impl Storage for WebStorage {
    fn read_text(&self, key: &str) -> Option<String> {
        let s = Self::storage()?;
        s.get_item(&self.full_key(key)).ok().flatten()
    }

    fn write_text(&self, key: &str, text: &str) -> std::io::Result<()> {
        let s = Self::storage().ok_or_else(io_err)?;
        s.set_item(&self.full_key(key), text).map_err(|_| io_err())
    }

    fn read_bytes(&self, key: &str) -> Option<Vec<u8>> {
        let text = self.read_text(key)?;
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .ok()
    }

    fn write_bytes(&self, key: &str, bytes: &[u8]) -> std::io::Result<()> {
        let enc = base64::engine::general_purpose::STANDARD.encode(bytes);
        self.write_text(key, &enc)
    }

    fn exists(&self, key: &str) -> bool {
        Self::storage()
            .and_then(|s| s.get_item(&self.full_key(key)).ok())
            .flatten()
            .is_some()
    }

    fn remove(&self, key: &str) -> std::io::Result<()> {
        let s = Self::storage().ok_or_else(io_err)?;
        s.remove_item(&self.full_key(key)).map_err(|_| io_err())
    }
}

fn io_err() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, "web storage error")
}
