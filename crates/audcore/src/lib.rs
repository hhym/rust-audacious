//! audcore — Audacious 内核的 Rust 重写。
//!
//! 对应 C 版 `src/libaudcore`：播放列表模型、标签读取、配置持久化、
//! 插件元信息与播放列表插件、频谱分析、以及 audtool 用的控制协议。
//! 音频输出与解码由上层 `audacious-rs` 通过 `rodio`/`symphonia` 实现。

pub mod config;
pub mod ipc;
pub mod playlist;
pub mod plugin;
pub mod spectrum;
pub mod tags;

pub use config::{Config, Theme};
pub use playlist::{Entry, Playlist, RepeatMode};

/// 程序名，与可执行文件保持一致。
pub const APP_NAME: &str = "audacious-rs";
/// 版本号（按用户习惯从 5.0 起标）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// 默认 EQ 频段（Hz），与 Audacious 十段均衡器一致。
pub const EQ_BANDS: [f32; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
