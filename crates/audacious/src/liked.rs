//! 「喜欢」列表：本地文件和在线歌曲都可以收藏，存在 `<config>/audacious-rs/liked.json`。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LikedItem {
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub duration: f64,
    /// 本地文件路径
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// 在线：来自哪个插件（按名字找回）
    #[serde(default)]
    pub plugin: Option<String>,
    /// 在线：插件返回的原始 item
    #[serde(default)]
    pub item: Option<serde_json::Value>,
    #[serde(default)]
    pub added_at: u64,
}

impl LikedItem {
    pub fn subtitle(&self) -> String {
        if self.artist.is_empty() {
            if let Some(path) = &self.path {
                return path.display().to_string();
            }
        }
        self.artist.clone()
    }

    pub fn length_text(&self) -> String {
        if self.duration > 0.0 {
            audcore::config::format_time(self.duration)
        } else {
            "--:--".into()
        }
    }

    pub fn key(&self) -> String {
        match (&self.path, &self.plugin, self.item.as_ref().and_then(|i| i["id"].as_str())) {
            (Some(path), _, _) => format!("file:{}", path.display()),
            (_, Some(plugin), Some(id)) => format!("online:{plugin}:{id}"),
            _ => format!("title:{}:{}", self.title, self.artist),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Liked {
    items: Vec<LikedItem>,
}

impl Liked {
    pub fn path() -> PathBuf {
        audcore::config::data_dir().join("liked.json")
    }

    pub fn load() -> Liked {
        let mut liked = Liked::default();
        if let Ok(text) = std::fs::read_to_string(Liked::path()) {
            if let Ok(items) = serde_json::from_str::<Vec<LikedItem>>(&text) {
                liked.items = items;
            }
        }
        liked
    }

    pub fn save(&self) {
        let path = Liked::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.items) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn items(&self) -> &[LikedItem] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 已经收藏过（同一个文件 / 同一个在线 id）返回 false。
    pub fn add(&mut self, item: LikedItem) -> bool {
        let key = item.key();
        if self.items.iter().any(|i| i.key() == key) {
            return false;
        }
        self.items.push(item);
        self.save();
        true
    }

    pub fn remove(&mut self, index: usize) -> Option<LikedItem> {
        if index >= self.items.len() {
            return None;
        }
        let item = self.items.remove(index);
        self.save();
        Some(item)
    }

    #[allow(dead_code)]
    pub fn contains(&self, item: &LikedItem) -> bool {
        let key = item.key();
        self.items.iter().any(|i| i.key() == key)
    }
}
