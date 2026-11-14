//! 播放列表模型，对应 libaudcore/playlist.c 的结构化重写。
//!
//! 这里只保存数据与索引，不涉及音频；播放推进策略（随机、循环、上下曲）
//! 由 [`Playlist::advance`] 统一给出，UI 与 audtool 共享同一份状态。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tags;

/// 播放列表中一首曲目的元信息。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 时长（秒），未知为 None。
    pub length: Option<f64>,
    /// 容器/格式名，例如 "MP3"、"FLAC"。
    pub format: String,
}

impl Entry {
    /// 从磁盘文件构造条目，读取标签；标签缺失时退回文件名。
    pub fn from_path(path: &Path) -> Self {
        let meta = tags::read(path);
        Entry {
            path: path.to_path_buf(),
            title: meta.title,
            artist: meta.artist,
            album: meta.album,
            length: meta.length,
            format: meta.format,
        }
    }

    /// 列表里显示的标题。
    pub fn display_title(&self) -> String {
        if self.title.trim().is_empty() {
            self.path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| self.path.to_string_lossy().to_string())
        } else {
            self.title.clone()
        }
    }

    pub fn display_artist(&self) -> String {
        if self.artist.trim().is_empty() {
            "—".into()
        } else {
            self.artist.clone()
        }
    }

    pub fn display_album(&self) -> String {
        if self.album.trim().is_empty() {
            "—".into()
        } else {
            self.album.clone()
        }
    }

    pub fn length_text(&self) -> String {
        match self.length {
            Some(v) => crate::config::format_time(v),
            None => "--:--".into(),
        }
    }
}

/// 循环模式：关闭 / 单曲 / 列表。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    Track,
    All,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Off
    }
}

impl RepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::Track,
            RepeatMode::Track => RepeatMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RepeatMode::Off => "循环：关",
            RepeatMode::All => "循环：列表",
            RepeatMode::Track => "循环：单曲",
        }
    }
}

/// 播放列表本体。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub entries: Vec<Entry>,
    /// 光标所在行（UI 选中），不一定在播放。
    pub selected: Option<usize>,
    /// 正在播放的行。
    pub playing: Option<usize>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    /// 最近播放过的索引，随机模式用来避免连播重复。
    #[serde(default)]
    history: Vec<usize>,
}

impl Playlist {
    /// 带初始模式的构造器（history 为内部状态，不对外开放字段）。
    pub fn new_with(repeat: RepeatMode, shuffle: bool) -> Self {
        Playlist {
            repeat,
            shuffle,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn current(&self) -> Option<&Entry> {
        self.playing.and_then(|i| self.entries.get(i))
    }

    pub fn total_duration(&self) -> f64 {
        self.entries.iter().filter_map(|e| e.length).sum()
    }

    /// 追加一首，返回其索引。
    pub fn add(&mut self, entry: Entry) -> usize {
        self.entries.push(entry);
        self.entries.len() - 1
    }

    /// 追加一个文件；已在列表里则直接复用原索引。
    pub fn add_path(&mut self, path: &Path) -> usize {
        if let Some(i) = self
            .entries
            .iter()
            .position(|e| e.path == path && path.exists())
        {
            return i;
        }
        self.add(Entry::from_path(path))
    }

    /// 插入到指定位置。
    pub fn insert(&mut self, index: usize, entry: Entry) {
        let at = index.min(self.entries.len());
        self.entries.insert(at, entry);
        self.shift_after_insert(at);
    }

    fn shift_after_insert(&mut self, at: usize) {
        for slot in [&mut self.selected, &mut self.playing] {
            if let Some(i) = *slot {
                if i >= at {
                    *slot = Some(i + 1);
                }
            }
        }
        for h in self.history.iter_mut() {
            if *h >= at {
                *h += 1;
            }
        }
    }

    /// 删除一行，并修正索引。
    pub fn remove(&mut self, index: usize) -> Option<Entry> {
        if index >= self.entries.len() {
            return None;
        }
        let removed = self.entries.remove(index);
        let fix = |slot: &mut Option<usize>| {
            if let Some(i) = *slot {
                if i > index {
                    *slot = Some(i - 1);
                } else if i == index {
                    *slot = None;
                }
            }
        };
        fix(&mut self.selected);
        fix(&mut self.playing);
        self.history.retain(|h| *h != index);
        for h in self.history.iter_mut() {
            if *h > index {
                *h -= 1;
            }
        }
        Some(removed)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.selected = None;
        self.playing = None;
        self.history.clear();
    }

    /// 把第 from 行移动到第 to 行。
    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() || to >= self.entries.len() || from == to {
            return;
        }
        let e = self.entries.remove(from);
        self.entries.insert(to, e);
        // 索引整体重算，简单可靠。
        self.remap_after_move(from, to);
    }

    fn remap_after_move(&mut self, from: usize, to: usize) {
        let map = |i: usize| -> usize {
            if i == from {
                to
            } else if from < to && i > from && i <= to {
                i - 1
            } else if to < from && i >= to && i < from {
                i + 1
            } else {
                i
            }
        };
        for slot in [&mut self.selected, &mut self.playing] {
            if let Some(i) = *slot {
                *slot = Some(map(i));
            }
        }
        for h in self.history.iter_mut() {
            *h = map(*h);
        }
    }

    /// 标记开始播放某行。
    pub fn mark_playing(&mut self, index: usize) {
        self.playing = Some(index);
        self.selected = Some(index);
        self.history.push(index);
        let keep = self.entries.len().max(1);
        if self.history.len() > keep * 2 {
            let drop = self.history.len() - keep;
            self.history.drain(0..drop);
        }
    }

    /// 下一首/上一首的候选索引；返回 None 表示列表到头且不循环。
    pub fn advance(&mut self, forward: bool) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        if self.repeat == RepeatMode::Track {
            return self.playing.or(Some(0));
        }

        let Some(cur) = self.playing else {
            return Some(if forward { 0 } else { len - 1 });
        };

        if self.shuffle && len > 1 {
            let recent: Vec<usize> = self.history.iter().rev().take(len - 1).copied().collect();
            let candidate: Vec<usize> = (0..len)
                .filter(|i| *i != cur && !recent.contains(i))
                .collect();
            if !candidate.is_empty() {
                let pick = pseudo_random(candidate.len());
                return Some(candidate[pick]);
            }
        }

        if forward {
            if cur + 1 < len {
                Some(cur + 1)
            } else if self.repeat == RepeatMode::All {
                Some(0)
            } else {
                None
            }
        } else if cur > 0 {
            Some(cur - 1)
        } else if self.repeat == RepeatMode::All {
            Some(len - 1)
        } else {
            None
        }
    }
}

/// 无第三方随机库时的低成本随机数：用系统时间做种子。
fn pseudo_random(bound: usize) -> usize {
    if bound <= 1 {
        return 0;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    // splitmix64 步进
    let mut x = nanos ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x % bound as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry {
            path: PathBuf::from(name),
            title: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn advance_stops_at_end_without_repeat() {
        let mut pl = Playlist::default();
        for n in 0..3 {
            pl.add(entry(&format!("{n}.mp3")));
        }
        pl.mark_playing(2);
        assert_eq!(pl.advance(true), None);
    }

    #[test]
    fn advance_wraps_with_repeat_all() {
        let mut pl = Playlist {
            repeat: RepeatMode::All,
            ..Default::default()
        };
        for n in 0..3 {
            pl.add(entry(&format!("{n}.mp3")));
        }
        pl.mark_playing(2);
        assert_eq!(pl.advance(true), Some(0));
    }

    #[test]
    fn remove_fixes_indices() {
        let mut pl = Playlist::default();
        for n in 0..4 {
            pl.add(entry(&format!("{n}.mp3")));
        }
        pl.mark_playing(2);
        pl.selected = Some(3);
        pl.remove(1);
        assert_eq!(pl.playing, Some(1));
        assert_eq!(pl.selected, Some(2));
    }
}
