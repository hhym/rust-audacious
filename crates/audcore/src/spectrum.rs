//! 频谱分析，对应 Audacious 的 visualization 插件。
//!
//! 播放线程把最近的单声道采样写进环形缓冲，UI 每 50ms 取一次做 Goertzel
//! 频段能量计算，得到 10 根（可配置）跳动柱。

use std::collections::VecDeque;
use std::f32::consts::PI;

/// 环形采样缓冲。容量约 2048 个采样（44.1k 下 ~46ms）。
pub struct SpectrumBuffer {
    samples: VecDeque<f32>,
    capacity: usize,
}

impl Default for SpectrumBuffer {
    fn default() -> Self {
        Self::new(2048)
    }
}

impl SpectrumBuffer {
    pub fn new(capacity: usize) -> Self {
        SpectrumBuffer {
            samples: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, sample: f32) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample.clamp(-1.0, 1.0));
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// 取出窗口内的采样副本（按时间先后）。
    pub fn window(&self, n: usize) -> Vec<f32> {
        let skip = self.samples.len().saturating_sub(n);
        self.samples.iter().skip(skip).copied().collect()
    }

    /// 计算 log 分布的频段能量，返回 0.0-1.0 归一化值。
    pub fn bands(&self, band_count: usize, sample_rate: u32) -> Vec<f32> {
        let window = self.window(1024);
        compute_bands(&window, band_count, sample_rate)
    }
}

/// Goertzel 算法求某个频率的能量。
fn goertzel(samples: &[f32], freq: f32, sample_rate: u32) -> f32 {
    if samples.is_empty() || freq <= 0.0 {
        return 0.0;
    }
    let k = (samples.len() as f32 * freq / sample_rate as f32).round();
    let w = 2.0 * PI * k / samples.len() as f32;
    let coeff = 2.0 * w.cos();
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    for &x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    power.max(0.0).sqrt() / samples.len() as f32
}

/// 按对数分布把 40Hz - 16kHz 分成 `band_count` 段。
pub fn compute_bands(samples: &[f32], band_count: usize, sample_rate: u32) -> Vec<f32> {
    if samples.is_empty() || band_count == 0 {
        return vec![0.0; band_count];
    }
    let nyquist = sample_rate as f32 / 2.0;
    let low = 40.0f32;
    let high = 16000.0f32.min(nyquist * 0.95);
    let mut out = Vec::with_capacity(band_count);
    for i in 0..band_count {
        let t = i as f32 / band_count as f32;
        let t2 = (i + 1) as f32 / band_count as f32;
        let f0 = low * (high / low).powf(t);
        let f1 = low * (high / low).powf(t2);
        let center = (f0 * f1).sqrt();
        let mag = goertzel(samples, center, sample_rate);
        // 对数压缩 + 限幅，视觉上更接近真实频谱。
        let db = 20.0 * (mag + 1e-6).log10();
        let norm = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        out.push(norm);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_hits_expected_band() {
        let sr = 44100;
        let freq = 1000.0;
        let samples: Vec<f32> = (0..1024)
            .map(|i| {
                let t = i as f32 / sr as f32;
                (2.0 * PI * freq * t).sin() * 0.5
            })
            .collect();
        let bands = compute_bands(&samples, 10, sr);
        let peak = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        // 1kHz 应落在中低频段（第 4-6 段）
        assert!((3..=6).contains(&peak), "peak band = {peak}, bands={bands:?}");
    }

    #[test]
    fn silence_is_zero() {
        let bands = compute_bands(&vec![0.0; 512], 10, 44100);
        assert!(bands.iter().all(|v| *v <= 0.01));
    }
}
