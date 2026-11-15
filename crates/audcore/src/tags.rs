//! 标签读取，对应 libaudtag 的常用部分。
//!
//! 用 lofty 读取 title/artist/album、时长与容器格式；失败时退回扩展名推断。

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub length: Option<f64>,
    pub format: String,
}

/// 读取单个文件的元信息。任何一步失败都不会中断，只是字段留空。
pub fn read(path: &Path) -> Metadata {
    let mut meta = Metadata {
        format: format_from_ext(path),
        ..Default::default()
    };

    let tagged = match Probe::open(path).and_then(|p| p.read()) {
        Ok(t) => t,
        Err(err) => {
            log::debug!("标签读取失败 {}: {err}", path.display());
            return meta;
        }
    };


    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        meta.title = tag.title().map(|s| s.to_string()).unwrap_or_default();
        meta.artist = tag.artist().map(|s| s.to_string()).unwrap_or_default();
        meta.album = tag.album().map(|s| s.to_string()).unwrap_or_default();
    }

    let dur = tagged.properties().duration();
    if !dur.is_zero() {
        meta.length = Some(dur.as_secs_f64());
    }

    meta
}

/// 读第一张内嵌封面（ID3 APIC / FLAC PICTURE），解码交给界面做缩略图。
pub fn read_art(path: &Path) -> Option<Vec<u8>> {
    let tagged = Probe::open(path).ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let picture = tag.pictures().first()?;
    Some(picture.data().to_vec())
}

fn format_from_ext(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "mp3" => "MP3",
        "flac" => "FLAC",
        "ogg" | "oga" => "Ogg Vorbis",
        "opus" => "Opus",
        "wav" => "WAV",
        "m4a" | "mp4" | "aac" => "AAC/M4A",
        "mka" | "webm" => "Matroska",
        "aiff" | "aif" => "AIFF",
        _ => "音频",
    }
    .into()
}

/// 常见可播放扩展名（与 C 版 input 插件集合对应）。
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "ogg", "oga", "opus", "wav", "m4a", "aac", "mp4", "mka", "webm", "aiff", "aif",
];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            let e = e.to_string_lossy().to_lowercase();
            AUDIO_EXTENSIONS.contains(&e.as_str())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn extension_gate() {
        assert!(is_audio_file(&PathBuf::from("a.MP3")));
        assert!(!is_audio_file(&PathBuf::from("a.txt")));
    }
}
