//! 配置持久化，对应 libaudcore/audconfig.c。
//!
//! 用 JSON 存到平台配置目录，字段变更时用 `#[serde(default)]` 保持向后兼容。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::playlist::RepeatMode;

/// 界面主题。默认橘黄，另外提供中性浅色与中性深色两套。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 橘黄：暖色米底 + 橙色强调（默认）
    #[default]
    Amber,
    /// 浅色：中性灰白
    Light,
    /// 深色：中性炭黑
    Dark,
}

impl Theme {
    pub fn label(&self) -> &'static str {
        match self {
            Theme::Amber => "橘黄主题",
            Theme::Light => "浅色主题",
            Theme::Dark => "深色主题",
        }
    }

    pub fn label_en(&self) -> &'static str {
        match self {
            Theme::Amber => "Amber theme",
            Theme::Light => "Light theme",
            Theme::Dark => "Dark theme",
        }
    }

    /// 深色系（决定 egui 的 base visuals）
    pub fn is_dark(&self) -> bool {
        matches!(self, Theme::Dark)
    }

    pub fn all() -> [Theme; 3] {
        [Theme::Amber, Theme::Light, Theme::Dark]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    /// 音量 0.0 - 1.0
    pub volume: f32,
    pub repeat: RepeatMode,
    pub shuffle: bool,
    /// 均衡器开关
    pub eq_enabled: bool,
    /// 十段增益（dB），与 `EQ_BANDS` 一一对应
    pub eq_gains: Vec<f32>,
    /// 主题强调色
    pub accent: [u8; 3],
    /// 界面主题：橘黄（默认）/ 浅色 / 深色
    #[serde(default)]
    pub theme: Theme,
    /// 迷你模式
    pub mini: bool,
    /// 上次浏览目录
    #[serde(default)]
    pub last_dir: Option<PathBuf>,
    /// 歌词对齐：0 居左 / 1 居中 / 2 居右
    #[serde(default)]
    pub lyric_align: u8,
    /// 歌词模式：false 滚动歌词 / true 卡拉OK
    #[serde(default)]
    pub lyric_karaoke: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            volume: 0.8,
            repeat: RepeatMode::Off,
            shuffle: false,
            eq_enabled: false,
            eq_gains: vec![0.0; crate::EQ_BANDS.len()],
            accent: [235, 152, 52],
            theme: Theme::default(),
            mini: false,
            last_dir: dirs::audio_dir().or_else(dirs::home_dir),
            lyric_align: 0,
            lyric_karaoke: false,
        }
    }
}

impl Config {
    /// 配置文件路径：`<config>/audacious-rs/config.json`
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(crate::APP_NAME)
            .join("config.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                log::warn!("配置解析失败，使用默认值: {err}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn clamp(&mut self) {
        self.volume = self.volume.clamp(0.0, 1.0);
        while self.eq_gains.len() < crate::EQ_BANDS.len() {
            self.eq_gains.push(0.0);
        }
        self.eq_gains.truncate(crate::EQ_BANDS.len());
        for g in &mut self.eq_gains {
            *g = g.clamp(-24.0, 24.0);
        }
    }
}

/// 秒数转 `mm:ss` / `h:mm:ss`。
pub fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".into();
    }
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod lyric_view_tests {
    use super::Config;

    #[test]
    fn old_config_without_lyric_fields_uses_defaults() {
        // 老版本配置没有 lyric_align / lyric_karaoke，应当能读进来并落到默认值
        let json = r#"{
            "volume": 0.8,
            "repeat": "Off",
            "shuffle": false,
            "eq_enabled": false,
            "eq_gains": [],
            "accent": [235, 152, 52],
            "mini": false
        }"#;
        let config: Config = serde_json::from_str(json).expect("old config should parse");
        assert_eq!(config.lyric_align, 0, "默认居左");
        assert!(!config.lyric_karaoke, "默认滚动歌词");
    }

    #[test]
    fn lyric_view_fields_round_trip() {
        let mut config = Config::default();
        config.lyric_align = 2;
        config.lyric_karaoke = true;
        let json = serde_json::to_string(&config).expect("serialize");
        let back: Config = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.lyric_align, 2);
        assert!(back.lyric_karaoke);
    }
}
