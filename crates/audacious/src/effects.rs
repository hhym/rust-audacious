//! 音频效果与采样旁路。
//!
//! - [`EqSource`]：十段 peaking biquad 均衡器，对应 Audacious 的 equalizer 效果插件。
//! - [`TapSource`]：把播出去的采样分一路给频谱缓冲，对应 visualization 插件的输入。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audcore::spectrum::SpectrumBuffer;
use rodio::Source;

/// 十段 EQ 参数，播放线程与 UI 共享。
pub struct EqSettings {
    enabled: AtomicBool,
    /// 参数版本号，UI 改动后自增，播放中的源按需重算系数。
    version: AtomicU32,
    gains: Mutex<Vec<f32>>,
}

impl EqSettings {
    pub fn new(gains: Vec<f32>, enabled: bool) -> Self {
        EqSettings {
            enabled: AtomicBool::new(enabled),
            version: AtomicU32::new(0),
            gains: Mutex::new(gains),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        self.bump();
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_gains(&self, gains: &[f32]) {
        if let Ok(mut g) = self.gains.lock() {
            *g = gains.to_vec();
        }
        self.bump();
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (Vec<f32>, u32) {
        let gains = self.gains.lock().map(|g| g.clone()).unwrap_or_default();
        (gains, self.version.load(Ordering::Relaxed))
    }
}

/// 单个双二阶滤波器（Direct Form I）。
#[derive(Clone, Copy, Debug, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// peaking EQ 系数。
    fn peaking(freq: f32, sample_rate: f32, gain_db: f32, q: f32) -> Self {
        let nyquist = sample_rate / 2.0;
        let freq = freq.clamp(20.0, nyquist * 0.95);
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        Biquad {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            ..Default::default()
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// 把解码器（i16）的采样过一遍 EQ 再输出（i16）。
pub struct EqSource<S> {
    inner: S,
    settings: Arc<EqSettings>,
    sample_rate: f32,
    channels: u16,
    channel_index: u16,
    filters: Vec<Vec<Biquad>>,
    version: u32,
}

impl<S> EqSource<S>
where
    S: Source<Item = i16>,
{
    pub fn new(inner: S, settings: Arc<EqSettings>) -> Self {
        let channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(1) as f32;
        let (gains, version) = settings.snapshot();
        let mut filters = Vec::new();
        for _ in 0..channels {
            filters.push(build_filters(&gains, sample_rate));
        }
        EqSource {
            inner,
            settings,
            sample_rate,
            channels,
            channel_index: 0,
            filters,
            version,
        }
    }

    fn refresh(&mut self) {
        let (gains, version) = self.settings.snapshot();
        if version != self.version {
            self.version = version;
            for bank in self.filters.iter_mut() {
                *bank = build_filters(&gains, self.sample_rate);
            }
        }
    }
}

fn build_filters(gains: &[f32], sample_rate: f32) -> Vec<Biquad> {
    audcore::EQ_BANDS
        .iter()
        .enumerate()
        .map(|(i, freq)| {
            let gain = gains.get(i).copied().unwrap_or(0.0);
            Biquad::peaking(*freq, sample_rate, gain, 1.0)
        })
        .collect()
}

impl<S> Iterator for EqSource<S>
where
    S: Source<Item = i16>,
{
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let sample = self.inner.next()?;
        if !self.settings.enabled() {
            return Some(sample);
        }
        self.refresh();

        let mut value = sample as f32 / 32768.0;
        let channel = self.channel_index as usize;
        if let Some(bank) = self.filters.get_mut(channel) {
            for filter in bank.iter_mut() {
                value = filter.process(value);
            }
        }
        self.channel_index = (self.channel_index + 1) % self.channels;

        let out = (value.clamp(-1.0, 1.0) * 32767.0) as i16;
        Some(out)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for EqSource<S>
where
    S: Source<Item = i16>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate as u32
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        // 换位置时清掉滤波器历史，避免爆音。
        for bank in self.filters.iter_mut() {
            for f in bank.iter_mut() {
                f.x1 = 0.0;
                f.x2 = 0.0;
                f.y1 = 0.0;
                f.y2 = 0.0;
            }
        }
        self.inner.try_seek(pos)
    }
}

/// 采样旁路：把播放中的采样喂给频谱缓冲，同时原样输出。
pub struct TapSource<S> {
    inner: S,
    buffer: Arc<Mutex<SpectrumBuffer>>,
    channels: u16,
    channel_index: u16,
    accumulator: f32,
    /// 每 N 个采样写一次，降低锁竞争。
    counter: u32,
}

impl<S> TapSource<S>
where
    S: Source<Item = i16>,
{
    pub fn new(inner: S, buffer: Arc<Mutex<SpectrumBuffer>>) -> Self {
        let channels = inner.channels().max(1);
        TapSource {
            inner,
            buffer,
            channels,
            channel_index: 0,
            accumulator: 0.0,
            counter: 0,
        }
    }
}

impl<S> Iterator for TapSource<S>
where
    S: Source<Item = i16>,
{
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        let sample = self.inner.next()?;
        self.accumulator += sample as f32 / 32768.0;
        self.channel_index += 1;
        if self.channel_index >= self.channels {
            let mono = self.accumulator / self.channels as f32;
            self.accumulator = 0.0;
            self.channel_index = 0;
            self.counter += 1;
            if self.counter >= 2 {
                self.counter = 0;
                if let Ok(mut buf) = self.buffer.lock() {
                    buf.push(mono);
                }
            }
        }
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for TapSource<S>
where
    S: Source<Item = i16>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_eq_keeps_signal() {
        let mut f = Biquad::peaking(1000.0, 44100.0, 0.0, 1.0);
        let y = f.process(0.5);
        assert!(y.abs() < 0.5 + 1e-3, "{y}");
    }

    #[test]
    fn boost_raises_energy() {
        let mut filters = build_filters(&[6.0; 10], 44100.0);
        let mut energy_in = 0.0f64;
        let mut energy_out = 0.0f64;
        for i in 0..2048 {
            let x = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44100.0).sin() * 0.2;
            let mut y = x;
            for f in filters.iter_mut() {
                y = f.process(y);
            }
            energy_in += (x as f64).powi(2);
            energy_out += (y as f64).powi(2);
        }
        assert!(energy_out > energy_in);
    }
}
