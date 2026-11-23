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
use rodio::source::SeekError;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use symphonia::core::audio::{SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions};
use symphonia::core::formats::{FormatOptions, FormatReader, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
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

        cleanup_transcode_cache();
        if let Some(p) = ffmpeg_path() {
            log::info!("FFmpeg 解码插件已就绪：{}", p.display());
        } else {
            log::info!("FFmpeg 解码插件未启用（未找到 ffmpeg.exe）");
        }

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

/// 基于 symphonia 的自有解码源。
///
/// 不用 rodio 自带的 `Decoder`：它把输入包成 `byte_len()` 未知的适配器，
/// MP4/M4A（iTunes 试听、常见在线音源）因此在探测阶段就触发 SeekError 崩溃。
/// 这里直接把 `File` 交给 symphonia（它知道文件长度），并实现 `Source`。
struct SymSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymDecoder>,
    track_id: u32,
    spec: SignalSpec,
    buffer: Vec<i16>,
    index: usize,
    finished: bool,
    duration: Option<Duration>,
}

impl SymSource {
    fn open(path: &Path) -> anyhow::Result<SymSource> {
        let file = std::fs::File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )?;
        let track = probed
            .format
            .default_track()
            .ok_or_else(|| anyhow::anyhow!("没有音频轨道"))?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default())?;
        let duration = params
            .time_base
            .zip(params.n_frames)
            .map(|(base, frames)| base.calc_time(frames))
            .map(|t| Duration::from_secs_f64(t.seconds as f64 + t.frac));

        let fallback_rate = params.sample_rate.unwrap_or(44100);
        let fallback_channels = params
            .channels
            .unwrap_or(symphonia::core::audio::Channels::FRONT_LEFT
                | symphonia::core::audio::Channels::FRONT_RIGHT);

        let mut source = SymSource {
            format: probed.format,
            decoder,
            track_id,
            spec: SignalSpec::new(fallback_rate, fallback_channels),
            buffer: Vec::new(),
            index: 0,
            finished: false,
            duration,
        };
        source.fill();
        Ok(source)
    }

    /// 解一个包填进缓冲区；返回 false 表示已到结尾。
    fn fill(&mut self) -> bool {
        if self.finished {
            return false;
        }
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.spec = *decoded.spec();
                    let capacity = decoded.capacity() as u64;
                    let mut samples = SampleBuffer::<i16>::new(capacity, self.spec);
                    samples.copy_interleaved_ref(decoded);
                    self.buffer.clear();
                    self.buffer.extend_from_slice(samples.samples());
                    self.index = 0;
                    return true;
                }
                Err(_) => continue,
            }
        }
    }
}

impl Iterator for SymSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        loop {
            if self.index < self.buffer.len() {
                let sample = self.buffer[self.index];
                self.index += 1;
                return Some(sample);
            }
            if !self.fill() {
                return None;
            }
        }
    }
}

impl Source for SymSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.spec.channels.count() as u16
    }

    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        let seeked = self
            .format
            .seek(
                symphonia::core::formats::SeekMode::Accurate,
                SeekTo::Time {
                    time: pos.as_secs_f64().into(),
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| SeekError::Other(Box::new(e)))?;
        let _ = seeked;
        self.finished = false;
        self.buffer.clear();
        self.index = 0;
        self.fill();
        Ok(())
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
        let (mut decoder, _played, _ff) = open_with_fallback(&path)?;
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

        // 在线音源：先落到本地缓存，再走正常的解码路径
        let media_path = match local_media_path(&entry.path) {
            Ok(path) => path,
            Err(err) => {
                self.fail(&entry.title, &format!("无法获取在线音源：{err}"));
                return;
            }
        };

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

        match self.try_device_start(&media_path) {
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
                match NullPlayer::start(media_path.clone(), self.spectrum.clone()) {
                    Ok(player) => {
                        sample_rate = player.rate;
                        length = tagged_length(&entry, player.length);
                        format = path_format(&media_path);
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

        let (decoder, _played, used_ffmpeg) = open_with_fallback(path)?;
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
                format: if used_ffmpeg {
                    format!("{} · FFmpeg", path_format(path))
                } else {
                    path_format(path)
                },
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
/// 本地文件直接用；`http(s)://` 开头的先下载到缓存。
fn local_media_path(path: &Path) -> anyhow::Result<PathBuf> {
    let text = path.to_string_lossy();
    if text.starts_with("http://") || text.starts_with("https://") {
        return audplugins::fetch_to_cache(&text);
    }
    // 插件调用 ffmpeg 转码后常常返回 file:///C:/... 这类本地地址
    if let Some(rest) = text.strip_prefix("file://") {
        let mut p = rest.replace("%20", " ").to_string();
        if cfg!(windows) && p.starts_with('/') {
            p.remove(0);
        }
        return Ok(PathBuf::from(p));
    }
    Ok(path.to_path_buf())
}

fn tagged_length(entry: &audcore::Entry, decoder_len: f64) -> f64 {
    match entry.length {
        Some(v) if v > 0.5 => v,
        _ => decoder_len,
    }
}

// ---------------------------------------------------------------------------
// FFmpeg 解码兜底：symphonia 不支持的格式（APE / WMA / DSD / AC3 …）交给 ffmpeg
// ---------------------------------------------------------------------------

/// 这些扩展名 symphonia 基本解不了，直接走 ffmpeg。
const FFMPEG_EXTS: &[&str] = &[
    "ape", "wma", "wmv", "asf", "dts", "ac3", "eac3", "dsf", "dff", "tak", "tta", "amr",
    "mka", "mpc", "ofr", "xm", "mod", "s3m", "it", "au", "spx", "caf",
];

/// 程序目录（exe 所在目录）：随包组件与「卸载」标记都放这里。
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_path_buf()))
}

/// 应用内置的 ffmpeg 安装位置：`<程序目录>\\ffmpeg\\ffmpeg.exe`。
pub fn ffmpeg_managed_path() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("ffmpeg").join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }))
}

/// 「已卸载」标记：设置页点了卸载后写好，避免又去 PATH 里捡到系统的 ffmpeg。
fn ffmpeg_disabled_marker() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("ffmpeg.disabled"))
}

/// 解码插件当前是否被用户在设置页关掉（卸载）。
pub fn ffmpeg_disabled() -> bool {
    ffmpeg_disabled_marker().map(|p| p.is_file()).unwrap_or(false)
}

/// 找 ffmpeg：环境变量 AUDACIOUS_FFMPEG → 程序目录（含 ffmpeg\\ffmpeg.exe）→ PATH。
pub fn ffmpeg_path() -> Option<PathBuf> {
    if ffmpeg_disabled() {
        return None;
    }
    if let Ok(p) = std::env::var("AUDACIOUS_FFMPEG") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_path_buf()))
    {
        for rel in [
            "ffmpeg.exe",
            "ffmpeg",
            "ffmpeg/bin/ffmpeg.exe",
            "ffmpeg/ffmpeg.exe",
            "ffmpeg/bin/ffmpeg",
        ] {
            let p = dir.join(rel);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            let p = d.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// 给设置页/插件页显示的一行状态。
pub fn ffmpeg_status() -> (bool, String) {
    match ffmpeg_path() {
        Some(p) => (true, format!("FFmpeg 解码可用 · {}", p.display())),
        None => (
            false,
            "FFmpeg 解码插件未安装：点「安装」下载，或把 ffmpeg.exe 放到程序目录".to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// 解码插件（FFmpeg）的安装 / 卸载
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct FfmpegJob {
    running: bool,
    message: String,
    done: u64,
    total: u64,
}

static FFMPEG_JOB: Mutex<Option<FfmpegJob>> = Mutex::new(None);

fn set_job(f: impl FnOnce(&mut FfmpegJob)) {
    let mut guard = match FFMPEG_JOB.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    let job = guard.get_or_insert_with(FfmpegJob::default);
    f(job);
}

/// 设置页轮询：(是否正在安装, 进度 0~1, 提示文字)
pub fn ffmpeg_job_state() -> (bool, f32, String) {
    let guard = match FFMPEG_JOB.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    match guard.as_ref() {
        Some(j) => {
            let p = if j.total > 0 { (j.done as f32 / j.total as f32).clamp(0.0, 1.0) } else { 0.0 };
            (j.running, p, j.message.clone())
        }
        None => (false, 0.0, String::new()),
    }
}

/// 卸载：删掉程序目录里随包的 ffmpeg，并写「已卸载」标记。
/// 只删应用自己的目录，绝不动 PATH 里系统装的 ffmpeg。
pub fn ffmpeg_uninstall() -> String {
    let Some(path) = ffmpeg_path() else {
        return "解码插件本来就没装".to_string();
    };
    let managed = ffmpeg_managed_path().and_then(|m| m.parent().map(|d| d.to_path_buf()));
    let inside = managed.as_ref().map(|d| path.starts_with(d)).unwrap_or(false);
    if inside {
        if let Some(dir) = &managed {
            let _ = std::fs::remove_dir_all(dir);
        }
        if let Some(p) = exe_dir().map(|d| d.join("ffmpeg.exe")) {
            if p.is_file() {
                let _ = std::fs::remove_file(p);
            }
        }
        if let Some(m) = ffmpeg_disabled_marker() {
            let _ = std::fs::write(&m, "disabled");
        }
        "已卸载 FFmpeg 解码插件（文件已删除，可随时重新安装）".to_string()
    } else {
        if let Some(m) = ffmpeg_disabled_marker() {
            let _ = std::fs::write(&m, "disabled");
        }
        format!("已停用 FFmpeg 解码插件；{} 是系统里的文件，没有删除", path.display())
    }
}

/// 安装：从 BtbN 的 FFmpeg-Builds 下载 win64 包，抽出 bin/ffmpeg.exe 放进
/// `<程序目录>\\ffmpeg\\ffmpeg.exe`。后台线程跑，进度写进 FFMPEG_JOB。
pub fn ffmpeg_install_start() {
    {
        let guard = match FFMPEG_JOB.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if guard.as_ref().map(|j| j.running).unwrap_or(false) {
            return;
        }
    }
    set_job(|j| {
        j.running = true;
        j.done = 0;
        j.total = 0;
        j.message = "准备下载…".to_string();
    });
    std::thread::Builder::new()
        .name("ffmpeg-install".into())
        .spawn(|| {
            let result = install_ffmpeg_impl();
            set_job(|j| match result {
                Ok(msg) => {
                    j.running = false;
                    j.message = msg;
                    j.total = 0;
                    j.done = 0;
                }
                Err(e) => {
                    j.running = false;
                    j.message = format!("安装失败：{e}");
                    j.total = 0;
                    j.done = 0;
                }
            });
        })
        .ok();
}

fn install_ffmpeg_impl() -> anyhow::Result<String> {
    // 先摘掉「已卸载」标记：如果系统里本来就有 ffmpeg，直接重新启用，省得下一百多兆。
    if let Some(m) = ffmpeg_disabled_marker() {
        let _ = std::fs::remove_file(m);
    }
    if let Some(p) = ffmpeg_path() {
        return Ok(format!("已重新启用解码插件 · {}", p.display()));
    }
    if !cfg!(windows) {
        anyhow::bail!("自动安装目前只支持 Windows；Linux/macOS 请用系统包管理器装 ffmpeg");
    }
    let dir = exe_dir().ok_or_else(|| anyhow::anyhow!("找不到程序目录"))?;
    let target_dir = dir.join("ffmpeg");
    std::fs::create_dir_all(&target_dir)?;

    let url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
    let zip_path = std::env::temp_dir().join("audacious-rs-ffmpeg-install.zip");

    set_job(|j| j.message = "正在下载 FFmpeg…（约 150 MB）".to_string());
    let mut resp = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?
        .get(url)
        .send()?;
    if !resp.status().is_success() {
        anyhow::bail!("下载返回 {}", resp.status());
    }
    let total = resp.content_length().unwrap_or(0);
    set_job(|j| j.total = total);
    let mut file = std::fs::File::create(&zip_path)?;
    let mut done = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    {
        use std::io::{Read, Write};
        loop {
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
            done += n as u64;
            set_job(|j| {
                j.done = done;
                j.message = format!("正在下载 FFmpeg… {:.1} MB", done as f64 / 1_048_576.0);
            });
        }
    }
    drop(file);

    set_job(|j| j.message = "正在解包…".to_string());
    let f = std::fs::File::open(&zip_path)?;
    let mut archive = zip::ZipArchive::new(f)?;
    let mut wanted = None;
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name().to_string();
        if name.ends_with("/bin/ffmpeg.exe") || name == "bin/ffmpeg.exe" {
            wanted = Some(i);
            break;
        }
    }
    let idx = wanted.ok_or_else(|| anyhow::anyhow!("压缩包里没找到 bin/ffmpeg.exe"))?;
    let out_path = target_dir.join("ffmpeg.exe");
    {
        let mut entry = archive.by_index(idx)?;
        let mut out = std::fs::File::create(&out_path)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    let _ = std::fs::remove_file(&zip_path);
    if let Some(p) = exe_dir().map(|d| d.join("ffmpeg.disabled")) {
        let _ = std::fs::remove_file(p);
    }
    Ok(format!("安装完成 · {}", out_path.display()))
}

fn needs_ffmpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| FFMPEG_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn transcode_cache_dir() -> PathBuf {
    std::env::temp_dir().join("audacious-rs-ffmpeg")
}

/// 顺手清掉 7 天前的转码缓存，别把临时目录塞满。
pub fn cleanup_transcode_cache() {
    let dir = transcode_cache_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if matches!(now.duration_since(modified), Ok(age) if age > Duration::from_secs(7 * 24 * 3600)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 用 ffmpeg 把任意格式转成 PCM WAV，返回缓存文件路径（同一文件只转一次）。
fn transcode(path: &Path) -> anyhow::Result<PathBuf> {
    use std::hash::{Hash, Hasher};
    let bin = ffmpeg_path().ok_or_else(|| anyhow::anyhow!("没有找到 ffmpeg"))?;
    let meta = std::fs::metadata(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    if let Ok(m) = meta.modified() {
        if let Ok(d) = m.duration_since(std::time::UNIX_EPOCH) {
            d.as_secs().hash(&mut hasher);
        }
    }
    let key = format!("{:016x}", hasher.finish());

    let dir = transcode_cache_dir();
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(format!("{key}.wav"));
    if let Ok(m) = std::fs::metadata(&out) {
        if m.len() > 44 {
            return Ok(out);
        }
    }

    log::info!("FFmpeg 转码：{} → {}", path.display(), out.display());
    let output = std::process::Command::new(&bin)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(path)
        .arg("-vn")
        .arg("-map")
        .arg("0:a:0")
        .arg("-c:a")
        .arg("pcm_s16le")
        .arg(&out)
        .output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&out);
        anyhow::bail!("ffmpeg 转码失败：{}", err.trim().lines().last().unwrap_or("未知错误"));
    }
    Ok(out)
}

/// 先按原文件开，失败（或格式明显 symphonia 不支持）再用 ffmpeg 转码后开。
/// 返回（解码源、实际播放路径、是否用了 ffmpeg）
fn open_with_fallback(path: &Path) -> anyhow::Result<(SymSource, PathBuf, bool)> {
    if !needs_ffmpeg(path) {
        match SymSource::open(path) {
            Ok(src) => return Ok((src, path.to_path_buf(), false)),
            Err(err) => {
                log::warn!("symphonia 打不开 {}（{err}），试试 ffmpeg", path.display());
            }
        }
    }
    let wav = transcode(path)?;
    let src = SymSource::open(&wav)?;
    Ok((src, wav, true))
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
