//! 扫描缓存：插件跑过一次后，把 `platform / version / methods` 存起来。
//!
//! 下次扫描按「路径 + 修改时间 + 大小」命中就直接用缓存，不再执行插件代码，
//! 这样插件目录里堆几十个插件也不会每次启动都卡住。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub platform: String,
    pub version: String,
    pub author: String,
    pub methods: Vec<String>,
    pub error: Option<String>,
}

pub(crate) fn cache_path() -> PathBuf {
    audcore::config::data_dir().join("plugin-cache.json")
}

/// 用文件/目录的修改时间和大小做键，文件一改缓存自然失效。
pub(crate) fn key(path: &Path) -> String {
    let meta = std::fs::metadata(path).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let size = meta.map(|m| m.len()).unwrap_or(0);
    format!("{}|{}|{}", path.display(), mtime, size)
}

pub(crate) fn load_all() -> HashMap<String, Entry> {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn save(map: &HashMap<String, Entry>) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(path, text);
    }
}
