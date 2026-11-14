//! audtool：命令行控制 audacious-rs，接口与 Audacious 官方 audtool 对齐（子集）。

use anyhow::Result;
use audcore::ipc::{call, Request};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "audtool", version, about = "控制正在运行的 audacious-rs")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 播放状态：playing / paused / stopped / error
    PlaybackStatus,
    /// 当前曲名
    CurrentSong,
    /// 当前曲目完整信息
    CurrentInfo,
    /// 播放列表条目数
    PlaylistCount,
    /// 播放列表第 N 首（从 1 开始）
    PlaylistSong { position: u64 },
    /// 播放第 N 首（从 1 开始）
    PlaylistPlay { position: u64 },
    /// 写播放列表到文件（.m3u/.pls）
    PlaylistSave { path: String },
    /// 从文件读播放列表
    PlaylistLoad { path: String },
    /// 往列表加文件/目录/播放列表
    PlaylistAdd { paths: Vec<String> },
    /// 清空播放列表
    PlaylistClear,
    /// 随机开关：on/off/空=toggle
    PlaylistShuffle { state: Option<String> },
    /// 循环：off/all/track/空=下一个模式
    PlaylistRepeat { state: Option<String> },
    /// 播放 / 暂停 切换
    PlayPause,
    /// 播放（可带行号）
    Play { position: Option<u64> },
    Pause,
    Stop,
    Next,
    Previous,
    /// 跳到第 N 秒
    Seek { seconds: f64 },
    /// 相对当前位置快进/快退
    SeekRelative { seconds: f64 },
    /// 读音量（0-100）
    GetVolume,
    /// 设音量（0-100）
    Volume { percent: f64 },
    /// 均衡器开关：on/off/空=toggle
    EqEnable { state: Option<String> },
    /// 设某段增益：band（1-10） gain（dB）
    EqGain { band: u64, gain: f64 },
    /// 版本号
    Version,
    /// 让播放器退出
    Quit,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Some(command) = cli.command else {
        // 没有子命令时打印状态摘要
        let status = call(Request::new("status", vec![]))?;
        let song = call(Request::new("current-song", vec![]))?;
        let count = call(Request::new("playlist-count", vec![]))?;
        let title = display(&song.data);
        println!(
            "{} · {} · 列表 {} 首",
            display(&status.data),
            if title.is_empty() { "—".to_string() } else { title },
            display(&count.data)
        );
        return Ok(());
    };

    let request = match command {
        Commands::PlaybackStatus => Request::new("playback-status", vec![]),
        Commands::CurrentSong => Request::new("current-song", vec![]),
        Commands::CurrentInfo => Request::new("current-info", vec![]),
        Commands::PlaylistCount => Request::new("playlist-count", vec![]),
        Commands::PlaylistSong { position } => {
            Request::new("playlist-song", vec![json!(position)])
        }
        Commands::PlaylistPlay { position } => {
            Request::new("playlist-play", vec![json!(position)])
        }
        Commands::PlaylistSave { path } => Request::new("playlist-save", vec![json!(path)]),
        Commands::PlaylistLoad { path } => Request::new("playlist-load", vec![json!(path)]),
        Commands::PlaylistAdd { paths } => {
            Request::new("playlist-add", paths.into_iter().map(Value::String).collect())
        }
        Commands::PlaylistClear => Request::new("playlist-clear", vec![]),
        Commands::PlaylistShuffle { state } => Request::new(
            "playlist-shuffle",
            state.map(Value::String).into_iter().collect(),
        ),
        Commands::PlaylistRepeat { state } => Request::new(
            "playlist-repeat",
            state.map(Value::String).into_iter().collect(),
        ),
        Commands::PlayPause => Request::new("play-pause", vec![]),
        Commands::Play { position } => {
            Request::new("play", position.map(|p| json!(p)).into_iter().collect())
        }
        Commands::Pause => Request::new("pause", vec![]),
        Commands::Stop => Request::new("stop", vec![]),
        Commands::Next => Request::new("next", vec![]),
        Commands::Previous => Request::new("prev", vec![]),
        Commands::Seek { seconds } => Request::new("seek", vec![json!(seconds)]),
        Commands::SeekRelative { seconds } => {
            Request::new("seek-relative", vec![json!(seconds)])
        }
        Commands::GetVolume => Request::new("get-volume", vec![]),
        Commands::Volume { percent } => Request::new("volume", vec![json!(percent)]),
        Commands::EqEnable { state } => {
            Request::new("eq-enable", state.map(Value::String).into_iter().collect())
        }
        Commands::EqGain { band, gain } => {
            Request::new("eq-gain", vec![json!(band.saturating_sub(1)), json!(gain)])
        }
        Commands::Version => Request::new("version", vec![]),
        Commands::Quit => Request::new("quit", vec![]),
    };

    let response = call(request)?;
    if !response.ok {
        anyhow::bail!(
            "{}",
            response.error.unwrap_or_else(|| "命令执行失败".into())
        );
    }
    let text = display(&response.data);
    if !text.is_empty() {
        println!("{text}");
    }
    Ok(())
}

fn display(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}
