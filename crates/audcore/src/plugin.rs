//! 插件接口，对应 libaudcore 的 plugin registry。
//!
//! C 版靠 `so` 动态库注册插件；Rust 版把插件抽象成 trait，
//! 编译期注册、运行期可用开关控制。真实生效的插件（解码、输出、效果）
//! 由上层 `audacious-rs` 实现，这里定义契约与播放列表格式插件。

use anyhow::{bail, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::playlist::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginKind {
    Input,
    Output,
    Effect,
    Visualization,
    Playlist,
    Transport,
}

impl PluginKind {
    pub fn label(self) -> &'static str {
        match self {
            PluginKind::Input => "输入",
            PluginKind::Output => "输出",
            PluginKind::Effect => "效果",
            PluginKind::Visualization => "可视化",
            PluginKind::Playlist => "播放列表",
            PluginKind::Transport => "传输",
        }
    }
}

/// 插件在设置窗口里展示的信息。
#[derive(Clone, Debug)]
pub struct PluginInfo {
    pub name: String,
    pub kind: PluginKind,
    pub description: String,
    pub enabled: bool,
}

impl PluginInfo {
    pub fn new(name: &str, kind: PluginKind, description: &str) -> Self {
        PluginInfo {
            name: name.into(),
            kind,
            description: description.into(),
            enabled: true,
        }
    }
}

/// 输入插件：声明可解码的扩展名。
pub trait InputPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn can_handle(&self, path: &Path) -> bool {
        path.extension()
            .map(|e| {
                let ext = e.to_string_lossy().to_lowercase();
                self.extensions().contains(&ext.as_str())
            })
            .unwrap_or(false)
    }
}

/// 效果插件（均衡器等）。
pub trait EffectPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
}

/// 播放列表格式插件（M3U / PLS ...）。
pub trait PlaylistPlugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn extensions(&self) -> &'static [&'static str];
    fn load(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn save(&self, path: &Path, entries: &[Entry]) -> Result<()>;
}

pub struct M3uPlugin;
pub struct PlsPlugin;

impl PlaylistPlugin for M3uPlugin {
    fn name(&self) -> &'static str {
        "M3U 播放列表"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["m3u", "m3u8"]
    }
    fn load(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let text = std::fs::read_to_string(path)?;
        let base = path.parent().unwrap_or(Path::new("."));
        let mut out = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let p = PathBuf::from(line);
            out.push(if p.is_absolute() { p } else { base.join(p) });
        }
        Ok(out)
    }
    fn save(&self, path: &Path, entries: &[Entry]) -> Result<()> {
        let mut text = String::from("#EXTM3U\n");
        for e in entries {
            text.push_str(&format!(
                "#EXTINF:{},{} - {}\n",
                e.length.unwrap_or(0.0).round() as i64,
                e.display_artist(),
                e.display_title()
            ));
            text.push_str(&e.path.to_string_lossy());
            text.push('\n');
        }
        std::fs::write(path, text)?;
        Ok(())
    }
}

impl PlaylistPlugin for PlsPlugin {
    fn name(&self) -> &'static str {
        "PLS 播放列表"
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["pls"]
    }
    fn load(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let text = std::fs::read_to_string(path)?;
        let base = path.parent().unwrap_or(Path::new("."));
        let mut items: Vec<(usize, PathBuf)> = Vec::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_lowercase();
            if let Some(idx) = key
                .strip_prefix("file")
                .and_then(|n| n.parse::<usize>().ok())
            {
                let p = PathBuf::from(value.trim());
                items.push((idx, if p.is_absolute() { p } else { base.join(p) }));
            }
        }
        items.sort_by_key(|(i, _)| *i);
        Ok(items.into_iter().map(|(_, p)| p).collect())
    }
    fn save(&self, path: &Path, entries: &[Entry]) -> Result<()> {
        let mut text = String::from("[playlist]\n");
        for (i, e) in entries.iter().enumerate() {
            let n = i + 1;
            text.push_str(&format!("File{n}={}\n", e.path.to_string_lossy()));
            text.push_str(&format!("Title{n}={}\n", e.display_title()));
            text.push_str(&format!(
                "Length{n}={}\n",
                e.length.unwrap_or(-1.0).round() as i64
            ));
        }
        text.push_str(&format!("NumberOfEntries={}\nVersion=2\n", entries.len()));
        std::fs::write(path, text)?;
        Ok(())
    }
}

/// 内置可用的播放列表插件集合。
pub fn playlist_plugins() -> Vec<&'static dyn PlaylistPlugin> {
    vec![&M3uPlugin, &PlsPlugin]
}

/// 按扩展名选插件。
pub fn playlist_plugin_for(path: &Path) -> Option<&'static dyn PlaylistPlugin> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    playlist_plugins()
        .into_iter()
        .find(|p| p.extensions().contains(&ext.as_str()))
}

/// 读取播放列表文件；不认识的格式直接报错。
pub fn load_playlist(path: &Path) -> Result<Vec<PathBuf>> {
    match playlist_plugin_for(path) {
        Some(p) => p.load(path),
        None => bail!("不支持的播放列表格式: {}", path.display()),
    }
}

pub fn save_playlist(path: &Path, entries: &[Entry]) -> Result<()> {
    match playlist_plugin_for(path) {
        Some(p) => p.save(path, entries),
        None => bail!("不支持的播放列表格式: {}", path.display()),
    }
}

/// 播放列表里出现过的扩展名，用于「支持的格式」展示。
pub fn playlist_extensions() -> BTreeSet<&'static str> {
    let mut set = BTreeSet::new();
    for p in playlist_plugins() {
        set.extend(p.extensions().iter().copied());
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn m3u_roundtrip() {
        let dir = std::env::temp_dir().join("audcore-m3u-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("list.m3u");

        let entries: Vec<Entry> = (0..3)
            .map(|i| Entry {
                path: PathBuf::from(format!("/music/{i}.mp3")),
                title: format!("Song {i}"),
                artist: "Artist".into(),
                length: Some(120.0),
                format: "MP3".into(),
                ..Default::default()
            })
            .collect();

        save_playlist(&file, &entries).unwrap();
        let loaded = load_playlist(&file).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[1], PathBuf::from("/music/1.mp3"));
    }
}
