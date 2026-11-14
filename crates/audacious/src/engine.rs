//! 播放引擎：对应 libaudcore 的 playback 线程与 output 抽象。
//!
//! 引擎跑在独立线程里，独占 rodio 的 `OutputStream`/`Sink`；
//! UI 与 audtool 只通过 [`Shared`] 发命令、读状态快照。
//!
//! 没有可用音频设备时（服务器、CI、声卡被独占）自动进入「静音解码」模式：
//! 仍然按实时速度解码、驱动频谱与进度，只是不出声，相当于 Audacious 的空输出插件。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use audcore::spectrum::SpectrumBuffer;
use audcore::{Config, Playlist};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};

use crate::effects::{EqSettings, EqSource, TapSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Stopped,
    Playing,
    Paused,
    Error,
}

impl Default for Status {
    fn default() -> Self {
        Status::Stopped
    }
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Stopped => "停止",
            Status::Playing => "播放中",
            Status::Paused => "暂停",
            Status::Error => "出错",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Stopped => "stopped",
            Status::Playing => "playing",
            Status::Paused => "paused",
            Status::Error => "error",
        }
    }
}

/// 给 UI / audtool 的状态快照。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackState {
    pub status: Status,
    pub position: f64,
    pub length: f64,
    pub volume: f32,
    pub current: Option<usize>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub sample_rate: u32,
    pub message: String,
}

impl Default for PlaybackState {
    fn default() -> Self {
        PlaybackState {
            status: Status::Stopped,
            position: 0.0,
            length: 0.0,
            volume: 0.8,
            current: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            format: String::new(),
            sample_rate: 44100,
            message: "就绪".into(),
        }
    }
}

/// 发往引擎线程的命令。
#[derive(Clone, Debug)]
pub enum Cmd {
    PlayIndex(usize),
    Toggle,
    Play,
    Pause,
    Stop,
    Next,
    Prev,
    Seek(f64),
    SeekRelative(f64),
    SetVolume(f32),
    /// UI/audtool 改了列表后通知刷新标题信息
    Refresh,
    Shutdown,
}

/// 进程内共享状态，UI 和 IPC 服务共用。
#[derive(Clone)]
pub struct Shared {
    pub playlist: Arc<Mutex<Playlist>>,
    pub state: Arc<Mutex<PlaybackState>>,
    pub config: Arc<Mutex<Config>>,
    pub eq: Arc<EqSettings>,
    pub spectrum: Arc<Mutex<SpectrumBuffer>>,
    /// audtool `quit` 置位，UI/无界面模式据此退出。
    pub shutdown: Arc<AtomicBool>,
    tx: Sender<Cmd>,
}

impl Shared {
    /// 启动引擎线程并返回共享句柄。
    pub fn spawn(config: Config) -> Shared {
        let playlist = Arc::new(Mutex::new(Playlist::new_with(
            config.repeat,
            config.shuffle,
        )));
        let state = Arc::new(Mutex::new(PlaybackState {
            volume: config.volume,
            ..Default::default()
        }));
        let eq = Arc::new(EqSettings::new(config.eq_gains.clone(), config.eq_enabled));
        let spectrum = Arc::new(Mutex::new(SpectrumBuffer::default()));

        let (tx, rx) = channel();
        let shared = Shared {
            playlist: playlist.clone(),
            state: state.clone(),
            config: Arc::new(Mutex::new(config)),
            eq: eq.clone(),
            spectrum: spectrum.clone(),
            shutdown: Arc::new(AtomicBool::new(false)),
            tx,
        };

        std::thread::Builder::new()
            .name("audacious-engine".into())
            .spawn(move || {
                let mut engine = Engine::new(rx, playlist, state, eq, spectrum);
                engine.run();
            })
            .expect("无法启动播放线程");

        shared
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn state(&self) -> PlaybackState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn playlist(&self) -> Playlist {
        self.playlist.lock().map(|p| p.clone()).unwrap_or_default()
    }
}

/// 无设备时的静音解码器，按实时速度喂频谱。
struct NullPlayer {
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    seek: Arc<Mutex<Option<f64>>>,
    frames: Arc<AtomicU64>,
    done: Arc<AtomicBool>,
    rate: u32,
    length: f64,
    handle: Option<JoinHandle<()>>,
}

impl NullPlayer {
    fn start(path: PathBuf, spectrum: Arc<Mutex<SpectrumBuffer>>) -> anyhow::Result<NullPlayer> {
        let file = std::fs::File::open(&path)?;
        let mut decoder = Decoder::new(file)?;
        let rate = decoder.sample_rate().max(1);
        let channels = decoder.channels().max(1) as usize;
        let length = decoder
            .total_duration()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let seek = Arc::new(Mutex::new(None::<f64>));
        let frames = Arc::new(AtomicU64::new(0));
        let done = Arc::new(AtomicBool::new(false));

        let (s_stop, s_paused, s_seek, s_frames, s_done) = (
            stop.clone(),
            paused.clone(),
            seek.clone(),
            frames.clone(),
            done.clone(),
        );
        let block = 1024usize;
        let block_secs = block as f64 / rate as f64;

        let handle = std::thread::Builder::new()
            .name("audacious-null-output".into())
            .spawn(move || {
                let mut accumulator = 0.0f32;
                let mut in_channel = 0usize;
                let mut deadline = Instant::now() + Duration::from_secs_f64(block_secs);

                loop {
                    if s_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Some(target) = s_seek.lock().ok().and_then(|mut g| g.take()) {
                        if decoder.try_seek(Duration::from_secs_f64(target)).is_ok() {
                            s_frames.store((target * rate as f64) as u64, Ordering::Relaxed);
                            deadline = Instant::now() + Duration::from_secs_f64(block_secs);
                        }
                    }
                    if s_paused.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(20));
                        deadline = Instant::now() + Duration::from_secs_f64(block_secs);
                        continue;
                    }

                    let mut produced = 0usize;
                    for _ in 0..block * channels {
                        match decoder.next() {
                            Some(sample) => {
                                accumulator += sample as f32 / 32768.0;
                                in_channel += 1;
                                if in_channel == channels {
                                    if let Ok(mut buf) = spectrum.lock() {
                                        buf.push(accumulator / channels as f32);
                                    }
                                    accumulator = 0.0;
                                    in_channel = 0;
                                }
                                produced += 1;
                            }
                            None => {
                                s_done.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                    }
                    let _ = produced;
                    s_frames.fetch_add(block as u64, Ordering::Relaxed);

                    let now = Instant::now();
                    if deadline > now {
                        std::thread::sleep(deadline - now);
                    }
                    deadline += Duration::from_secs_f64(block_secs);
                    if deadline < Instant::now() {
                        deadline = Instant::now() + Duration::from_secs_f64(block_secs);
                    }
                }
            })?;

        Ok(NullPlayer {
            stop,
            paused,
            seek,
            frames,
            done,
            rate,
            length,
            handle: Some(handle),
        })
    }

    fn position(&self) -> f64 {
        self.frames.load(Ordering::Relaxed) as f64 / self.rate as f64
    }

    fn finished(&self) -> bool {
        self.done.load(Ordering::Relaxed)
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    fn seek(&self, pos: f64) {
        if let Ok(mut slot) = self.seek.lock() {
            *slot = Some(pos);
        }
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Engine {
    rx: Receiver<Cmd>,
    playlist: Arc<Mutex<Playlist>>,
    state: Arc<Mutex<PlaybackState>>,
    eq: Arc<EqSettings>,
    spectrum: Arc<Mutex<SpectrumBuffer>>,
    volume: f32,
    output: Option<(OutputStream, OutputStreamHandle)>,
    /// 设备创建失败后不再重复尝试
    device_failed: bool,
    sink: Option<Sink>,
    null: Option<NullPlayer>,
}

impl Engine {
    fn new(
        rx: Receiver<Cmd>,
        playlist: Arc<Mutex<Playlist>>,
        state: Arc<Mutex<PlaybackState>>,
        eq: Arc<EqSettings>,
        spectrum: Arc<Mutex<SpectrumBuffer>>,
    ) -> Self {
        let volume = state.lock().map(|s| s.volume).unwrap_or(0.8);
        Engine {
            rx,
            playlist,
            state,
            eq,
            spectrum,
            volume,
            output: None,
            device_failed: false,
            sink: None,
            null: None,
        }
    }

    fn run(&mut self) {
        loop {
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Cmd::Shutdown) => break,
                Ok(cmd) => {
                    self.handle(cmd);
                    self.tick();
                }
                Err(RecvTimeoutError::Timeout) => self.tick(),
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        if let Some(null) = self.null.as_mut() {
            null.stop();
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PlayIndex(i) => self.start(i),
            Cmd::Toggle => match self.status() {
                Status::Playing => self.pause(),
                Status::Paused => self.resume(),
                _ => self.play_selected(),
            },
            Cmd::Play => match self.status() {
                Status::Paused => self.resume(),
                Status::Playing => {}
                _ => self.play_selected(),
            },
            Cmd::Pause => self.pause(),
            Cmd::Stop => self.stop(),
            Cmd::Next => self.step(true),
            Cmd::Prev => self.step(false),
            Cmd::Seek(pos) => self.seek(pos),
            Cmd::SeekRelative(delta) => {
                let pos = self.state.lock().map(|s| s.position).unwrap_or(0.0);
                self.seek(pos + delta);
            }
            Cmd::SetVolume(v) => {
                self.volume = v.clamp(0.0, 1.0);
                if let Some(sink) = &self.sink {
                    sink.set_volume(self.volume);
                }
                if let Ok(mut st) = self.state.lock() {
                    st.volume = self.volume;
                }
            }
            Cmd::Refresh => self.refresh_titles(),
            Cmd::Shutdown => {}
        }
    }

    fn status(&self) -> Status {
        self.state.lock().map(|s| s.status).unwrap_or(Status::Stopped)
    }

    fn play_selected(&mut self) {
        let index = {
            let pl = match self.playlist.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            pl.playing
                .or(pl.selected)
                .or(if pl.is_empty() { None } else { Some(0) })
        };
        if let Some(i) = index {
            self.start(i);
        }
    }

    fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        if let Some(null) = &self.null {
            null.pause();
        }
        if let Ok(mut st) = self.state.lock() {
            if st.status == Status::Playing {
                st.status = Status::Paused;
            }
        }
    }

    fn resume(&mut self) {
        if let Some(sink) = &self.sink {
            sink.play();
            if let Ok(mut st) = self.state.lock() {
                st.status = Status::Playing;
            }
        } else if let Some(null) = &self.null {
            null.resume();
            if let Ok(mut st) = self.state.lock() {
                st.status = Status::Playing;
            }
        } else {
            self.play_selected();
        }
    }

    fn stop(&mut self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        if let Some(null) = self.null.as_mut() {
            null.stop();
        }
        self.sink = None;
        self.null = None;
        if let Ok(mut buf) = self.spectrum.lock() {
            buf.clear();
        }
        if let Ok(mut pl) = self.playlist.lock() {
            pl.playing = None;
        }
        if let Ok(mut st) = self.state.lock() {
            st.status = Status::Stopped;
            st.position = 0.0;
            st.current = None;
            st.message = "已停止".into();
        }
    }

    fn step(&mut self, forward: bool) {
        let next = {
            let mut pl = match self.playlist.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            // 未播放时按方向从光标处起播
            if pl.playing.is_none() {
                pl.playing = pl.selected;
            }
            pl.advance(forward)
        };
        match next {
            Some(i) => self.start(i),
            None => self.stop_at_end(),
        }
    }

    fn stop_at_end(&mut self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
        if let Some(null) = self.null.as_mut() {
            null.stop();
        }
        self.sink = None;
        self.null = None;
        if let Ok(mut st) = self.state.lock() {
            st.status = Status::Stopped;
            st.position = st.length;
            st.message = "播放结束".into();
        }
    }

    fn seek(&mut self, pos: f64) {
        let length = self.state.lock().map(|s| s.length).unwrap_or(0.0);
        let target = pos.clamp(0.0, if length > 0.0 { length } else { pos.max(0.0) });
        if let Some(sink) = &self.sink {
            if sink.try_seek(Duration::from_secs_f64(target)).is_ok() {
                if let Ok(mut st) = self.state.lock() {
                    st.position = target;
                }
            }
        } else if let Some(null) = &self.null {
            null.seek(target);
            if let Ok(mut st) = self.state.lock() {
                st.position = target;
            }
        }
    }

    fn start(&mut self, index: usize) {
        let entry = {
            let pl = match self.playlist.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            pl.get(index).cloned()
        };
        let Some(entry) = entry else { return };

        if let Some(old) = self.sink.take() {
            old.stop();
        }
        if let Some(old) = self.null.as_mut() {
            old.stop();
        }
        self.null = None;
        if let Ok(mut buf) = self.spectrum.lock() {
            buf.clear();
        }
        if let Ok(mut pl) = self.playlist.lock() {
            pl.mark_playing(index);
        }

        let sample_rate;
        let length;
        let format;
        let message;

        match self.try_device_start(&entry.path) {
            Ok((sink, info)) => {
                self.sink = Some(sink);
                sample_rate = info.sample_rate;
                length = tagged_length(&entry, info.length);
                format = info.format;
                message = format!("正在播放 {}", entry.display_title());
                log::info!("play #{} {}", index, entry.path.display());
            }
            Err(err) => {
                // 设备失败 → 静音解码兜底
                log::warn!("音频输出不可用（{err}），改用静音解码");
                match NullPlayer::start(entry.path.clone(), self.spectrum.clone()) {
                    Ok(player) => {
                        sample_rate = player.rate;
                        length = tagged_length(&entry, player.length);
                        format = path_format(&entry.path);
                        message = format!("静音解码（无音频设备）：{}", entry.display_title());
                        self.null = Some(player);
                    }
                    Err(err) => {
                        self.fail(&entry.title, &format!("无法解码：{err}"));
                        return;
                    }
                }
            }
        }

        if let Ok(mut st) = self.state.lock() {
            st.status = Status::Playing;
            st.current = Some(index);
            st.position = 0.0;
            st.length = length;
            st.title = entry.display_title();
            st.artist = entry.display_artist();
            st.album = entry.display_album();
            st.format = format;
            st.sample_rate = sample_rate;
            st.message = message;
        }
    }

    fn try_device_start(&mut self, path: &Path) -> anyhow::Result<(Sink, StreamInfo)> {
        if self.output.is_none() && !self.device_failed {
            match OutputStream::try_default() {
                Ok((stream, handle)) => self.output = Some((stream, handle)),
                Err(err) => {
                    self.device_failed = true;
                    return Err(err.into());
                }
            }
        }
        let Some((_, handle)) = &self.output else {
            anyhow::bail!("没有音频输出设备");
        };

        let file = std::fs::File::open(path)?;
        let decoder = Decoder::new(file)?;
        let sample_rate = decoder.sample_rate();
        let length = decoder
            .total_duration()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        let eq_source = EqSource::new(decoder, self.eq.clone());
        let tapped = TapSource::new(eq_source, self.spectrum.clone());

        let sink = Sink::try_new(handle)?;
        sink.append(tapped);
        sink.set_volume(self.volume);
        sink.play();

        Ok((
            sink,
            StreamInfo {
                sample_rate,
                length,
                format: path_format(path),
            },
        ))
    }

    fn fail(&mut self, title: &str, message: &str) {
        if let Ok(mut st) = self.state.lock() {
            st.status = Status::Error;
            st.title = title.to_string();
            st.message = message.to_string();
        }
        log::warn!("{message}");
    }

    fn refresh_titles(&self) {
        let entry = {
            let pl = match self.playlist.lock() {
                Ok(p) => p,
                Err(_) => return,
            };
            pl.current().cloned()
        };
        if let (Some(entry), Ok(mut st)) = (entry, self.state.lock()) {
            st.title = entry.display_title();
            st.artist = entry.display_artist();
            st.album = entry.display_album();
        }
    }

    fn tick(&mut self) {
        let (status, device_length) = match self.state.lock() {
            Ok(st) => (st.status, st.length),
            Err(_) => return,
        };
        if status != Status::Playing {
            return;
        }

        let mut advance = false;
        if let Some(sink) = &self.sink {
            let pos = sink.get_pos().as_secs_f64();
            if let Ok(mut st) = self.state.lock() {
                st.position = pos;
            }
            advance = sink.empty() && (device_length <= 0.0 || pos >= device_length - 0.05);
        } else if let Some(null) = &self.null {
            let pos = null.position();
            if let Ok(mut st) = self.state.lock() {
                st.position = pos;
            }
            advance = null.finished();
        }

        if advance {
            self.step(true);
        }
    }
}

struct StreamInfo {
    sample_rate: u32,
    length: f64,
    format: String,
}

/// 标签里的时长更可信（rodio 对部分 wav 头会算错）。
fn tagged_length(entry: &audcore::Entry, decoder_len: f64) -> f64 {
    match entry.length {
        Some(v) if v > 0.5 => v,
        _ => decoder_len,
    }
}

fn path_format(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 2 秒的 44.1k 立体声正弦 wav 到临时目录。
    fn make_wav(dir: &Path, name: &str, seconds: u32) -> PathBuf {
        let path = dir.join(name);
        let rate = 44100u32;
        let channels = 2u16;
        let frames = rate * seconds;
        let mut bytes = Vec::new();
        // RIFF header
        bytes.extend_from_slice(b"RIFF");
        let data_len = frames * channels as u32 * 2;
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        for _ in 0..frames {
            let s = 8000i16;
            bytes.extend_from_slice(&s.to_le_bytes());
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("写测试 wav");
        path
    }

    #[test]
    fn null_player_advances_and_finishes() {
        let dir = std::env::temp_dir().join("audacious-null-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = make_wav(&dir, "tone.wav", 1);
        let spectrum = Arc::new(Mutex::new(SpectrumBuffer::default()));
        let mut player = NullPlayer::start(path, spectrum).expect("null player");
        std::thread::sleep(Duration::from_millis(300));
        let pos = player.position();
        assert!(pos > 0.05, "position should advance, got {pos}");
        assert!(!player.finished());
        player.stop();
    }
}
