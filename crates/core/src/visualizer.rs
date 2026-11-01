// SPDX-License-Identifier: GPL-3.0-or-later
//! The audio visualizer's numbers (v0.4 phase 3): band levels of whatever the desktop
//! is playing, shaped as a symmetrical waveform (the bands mirrored around the middle,
//! each bar drawn equally on both sides of a centre line), and a gate that shows it
//! only while there is sound. Pure: the app feeds samples and draws.

use std::f64::consts::PI;

/// Samples per second the app captures at (mono).
pub const RATE: f64 = 22_050.0;
/// Frequency bands per side; the waveform has twice as many bars.
pub const BANDS: usize = 12;
const WINDOW: usize = 1024;
const LOW_HZ: f64 = 60.0;
const HIGH_HZ: f64 = 8_000.0;

/// In-place radix-2 FFT of `re`/`im` (length a power of two).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (w_re, w_im) = ((angle * k as f64).cos(), (angle * k as f64).sin());
                let (a, b) = (start + k, start + k + len / 2);
                let (t_re, t_im) = (re[b] * w_re - im[b] * w_im, re[b] * w_im + im[b] * w_re);
                re[b] = re[a] - t_re;
                im[b] = im[a] - t_im;
                re[a] += t_re;
                im[a] += t_im;
            }
        }
        len <<= 1;
    }
}

pub struct Analyzer {
    /// The last `WINDOW` samples.
    ring: Vec<f32>,
    at: usize,
    bands: [f64; BANDS],
    /// Slowly falling loudest band seen, so quiet and loud music both fill the bars.
    gain: f64,
    rms: f64,
}

impl Default for Analyzer {
    fn default() -> Self {
        Analyzer { ring: vec![0.0; WINDOW], at: 0, bands: [0.0; BANDS], gain: 1e-3, rms: 0.0 }
    }
}

impl Analyzer {
    pub fn feed(&mut self, samples: &[f32]) {
        for &s in samples {
            self.ring[self.at] = s;
            self.at = (self.at + 1) % WINDOW;
        }
        if !samples.is_empty() {
            self.rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / samples.len() as f64).sqrt();
        }
    }

    /// Loudness of the last fed chunk (RMS, 0..1 for full-scale audio).
    pub fn rms(&self) -> f64 {
        self.rms
    }

    /// Recomputes the bands from the last window; call once per frame.
    pub fn update(&mut self) {
        let mut re: Vec<f64> = (0..WINDOW)
            .map(|i| {
                let hann = 0.5 - 0.5 * (2.0 * PI * i as f64 / (WINDOW - 1) as f64).cos();
                self.ring[(self.at + i) % WINDOW] as f64 * hann
            })
            .collect();
        let mut im = vec![0.0; WINDOW];
        fft(&mut re, &mut im);
        let hz_per_bin = RATE / WINDOW as f64;
        let mut raw = [0.0; BANDS];
        for (b, value) in raw.iter_mut().enumerate() {
            let edge = |k: f64| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(k / BANDS as f64);
            let (lo, hi) = ((edge(b as f64) / hz_per_bin) as usize, ((edge(b as f64 + 1.0) / hz_per_bin) as usize).max((edge(b as f64) / hz_per_bin) as usize + 1));
            let bins = lo.max(1)..hi.min(WINDOW / 2);
            let n = bins.len().max(1) as f64;
            *value = bins.map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt()).sum::<f64>() / n;
        }
        let loudest = raw.iter().copied().fold(0.0, f64::max);
        self.gain = (self.gain * 0.995).max(loudest).max(1e-3);
        for (band, value) in self.bands.iter_mut().zip(raw) {
            // Perceived loudness: a square root, then fast rise, slow fall.
            let target = (value / self.gain).sqrt().clamp(0.0, 1.0);
            let k = if target > *band { 0.6 } else { 0.18 };
            *band += (target - *band) * k;
        }
    }

    /// Lets the bars fall to rest (no new audio).
    pub fn decay(&mut self) {
        for band in &mut self.bands {
            *band *= 0.8;
        }
        self.rms = 0.0;
    }

    /// `2 × BANDS` bar heights 0..1, low frequencies in the middle, mirrored outwards.
    pub fn bars(&self) -> Vec<f64> {
        self.bands.iter().rev().chain(self.bands.iter()).copied().collect()
    }
}

/// Shows the visualizer only while there is sound: on at once when it starts, off after
/// a few seconds of silence (pauses between songs don't flicker it).
#[derive(Debug, Default)]
pub struct Gate {
    last_sound_us: Option<i64>,
}

/// Quieter than this counts as silence (about -54 dBFS).
pub const SILENCE: f64 = 0.002;
const HOLD_US: i64 = 2_500_000;

impl Gate {
    pub fn update(&mut self, rms: f64, now_us: i64) -> bool {
        if rms > SILENCE {
            self.last_sound_us = Some(now_us);
        }
        self.last_sound_us.is_some_and(|t| now_us - t < HOLD_US)
    }

    pub fn reset(&mut self) {
        self.last_sound_us = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (0.5 * (2.0 * PI * hz * i as f64 / RATE).sin()) as f32).collect()
    }

    #[test]
    fn a_tone_lights_its_band_and_mirrors() {
        let mut a = Analyzer::default();
        for _ in 0..8 {
            a.feed(&tone(1000.0, 1024));
            a.update();
        }
        let bars = a.bars();
        assert_eq!(bars.len(), 2 * BANDS);
        for i in 0..BANDS {
            assert_eq!(bars[BANDS - 1 - i], bars[BANDS + i], "symmetric");
        }
        let loudest = (0..BANDS).max_by(|x, y| a.bands[*x].total_cmp(&a.bands[*y])).unwrap();
        let edge = |k: f64| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(k / BANDS as f64);
        assert!(edge(loudest as f64) <= 1000.0 && 1000.0 <= edge(loudest as f64 + 1.0) * 1.1, "1 kHz lands in band {loudest}");
        assert!(a.rms() > 0.3);
    }

    #[test]
    fn silence_stays_flat_and_bars_fall() {
        let mut a = Analyzer::default();
        a.feed(&[0.0; 1024]);
        a.update();
        assert!(a.bars().iter().all(|b| *b < 0.05));
        a.feed(&tone(200.0, 1024));
        a.update();
        let before: f64 = a.bars().iter().sum();
        a.decay();
        assert!(a.bars().iter().sum::<f64>() < before);
    }

    #[test]
    fn gate_holds_through_short_gaps() {
        let mut g = Gate::default();
        assert!(!g.update(0.0, 0), "never shown before any sound");
        assert!(g.update(0.1, 1_000));
        assert!(g.update(0.0, 1_000 + 2_000_000), "a gap between songs");
        assert!(!g.update(0.0, 1_000 + HOLD_US), "hidden after the hold");
    }
}
