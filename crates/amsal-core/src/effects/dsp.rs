//! DSP filter chain — scroll-configured audio processing.
//!
//! Filters compose via the AudioFilter trait (same interface: &mut [f32], channels, rate).
//! The chain is built from a JSON scroll at `/amsal/playback/eq` and hot-swapped
//! via `Arc<RwLock<Option<DspChain>>>` in the cpal output callback.

use std::f32::consts::PI;

/// One audio filter operation. Process samples in-place.
pub trait AudioFilter: Send + Sync {
    fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32);
}

/// Biquad filter — the atom of audio DSP.
///
/// Peaking EQ mode: boost/cut at a center frequency with configurable Q.
/// Direct Form I implementation with per-channel state.
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    /// Per-channel delay lines: (x[n-1], x[n-2], y[n-1], y[n-2])
    state: Vec<[f32; 4]>,
}

impl Biquad {
    /// Create a peaking EQ biquad.
    pub fn peaking_eq(freq_hz: f32, gain_db: f32, q: f32, sample_rate: u32, channels: u16) -> Self {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq_hz / sample_rate as f32;
        let alpha = w0.sin() / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * w0.cos();
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * w0.cos();
        let a2 = 1.0 - alpha / a;

        // Normalize by a0
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            state: vec![[0.0; 4]; channels as usize],
        }
    }
}

impl AudioFilter for Biquad {
    fn process(&mut self, samples: &mut [f32], channels: u16, _sample_rate: u32) {
        let ch = channels as usize;
        if ch == 0 { return; }
        // Ensure state has enough channels
        while self.state.len() < ch {
            self.state.push([0.0; 4]);
        }
        for frame in samples.chunks_exact_mut(ch) {
            for (c, sample) in frame.iter_mut().enumerate() {
                let s = &mut self.state[c];
                let x = *sample;
                let y = self.b0 * x + self.b1 * s[0] + self.b2 * s[1]
                       - self.a1 * s[2] - self.a2 * s[3];
                s[1] = s[0]; // x[n-2] = x[n-1]
                s[0] = x;    // x[n-1] = x[n]
                s[3] = s[2]; // y[n-2] = y[n-1]
                s[2] = y;    // y[n-1] = y[n]
                *sample = y;
            }
        }
    }
}

/// Gain filter — multiplication.
pub struct Gain {
    factor: f32,
}

impl Gain {
    pub fn from_db(db: f32) -> Self {
        Self { factor: 10.0f32.powf(db / 20.0) }
    }
}

impl AudioFilter for Gain {
    fn process(&mut self, samples: &mut [f32], _channels: u16, _sample_rate: u32) {
        for s in samples.iter_mut() {
            *s *= self.factor;
        }
    }
}

/// Ordered chain of filters. Applied in sequence.
pub struct DspChain {
    filters: Vec<Box<dyn AudioFilter>>,
}

impl DspChain {
    pub fn new(filters: Vec<Box<dyn AudioFilter>>) -> Self {
        Self { filters }
    }

    pub fn process(&mut self, samples: &mut [f32], channels: u16, sample_rate: u32) {
        for filter in &mut self.filters {
            filter.process(samples, channels, sample_rate);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.filters.is_empty()
    }
}

/// Build a DspChain from scroll JSON.
///
/// Expected schema:
/// ```json
/// {"filters": [
///   {"type": "eq", "freq_hz": 80, "gain_db": 3.0, "q": 0.7},
///   {"type": "gain", "db": -1.5}
/// ]}
/// ```
pub fn chain_from_value(v: &serde_json::Value, sample_rate: u32, channels: u16) -> DspChain {
    let mut filters: Vec<Box<dyn AudioFilter>> = Vec::new();

    if let Some(arr) = v["filters"].as_array() {
        for spec in arr {
            match spec["type"].as_str() {
                Some("eq") => {
                    let freq = spec["freq_hz"].as_f64().unwrap_or(1000.0) as f32;
                    let gain = spec["gain_db"].as_f64().unwrap_or(0.0) as f32;
                    let q = spec["q"].as_f64().unwrap_or(0.707) as f32;
                    if freq > 0.0 && q > 0.0 {
                        filters.push(Box::new(Biquad::peaking_eq(freq, gain, q, sample_rate, channels)));
                    }
                }
                Some("gain") => {
                    let db = spec["db"].as_f64().unwrap_or(0.0) as f32;
                    filters.push(Box::new(Gain::from_db(db)));
                }
                _ => {} // Unknown filter type — skip
            }
        }
    }

    DspChain::new(filters)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_doubles_amplitude() {
        let mut gain = Gain::from_db(6.0); // ~2x
        let mut samples = vec![0.5, -0.5, 0.25, -0.25];
        gain.process(&mut samples, 2, 44100);
        // 6 dB ≈ 1.995x
        assert!((samples[0] - 0.5 * 1.9953).abs() < 0.01);
        assert!((samples[1] - (-0.5 * 1.9953)).abs() < 0.01);
    }

    #[test]
    fn chain_from_value_empty() {
        let v = serde_json::json!({"filters": []});
        let chain = chain_from_value(&v, 44100, 2);
        assert!(chain.is_empty());
    }

    #[test]
    fn chain_from_value_eq_and_gain() {
        let v = serde_json::json!({
            "filters": [
                {"type": "eq", "freq_hz": 1000, "gain_db": 0.0, "q": 0.707},
                {"type": "gain", "db": 0.0}
            ]
        });
        let chain = chain_from_value(&v, 44100, 2);
        assert!(!chain.is_empty());
    }

    #[test]
    fn biquad_unity_gain() {
        // 0 dB gain EQ should pass signal nearly unchanged
        let mut biquad = Biquad::peaking_eq(1000.0, 0.0, 0.707, 44100, 1);
        let original = vec![0.5; 64];
        let mut samples = original.clone();
        biquad.process(&mut samples, 1, 44100);
        // After settling, output should be very close to input
        for (i, s) in samples.iter().enumerate().skip(4) {
            assert!(
                (s - original[i]).abs() < 0.01,
                "sample {} diverged: {} vs {}",
                i, s, original[i]
            );
        }
    }

    #[test]
    fn dsp_chain_applies_in_order() {
        let v = serde_json::json!({
            "filters": [
                {"type": "gain", "db": 6.0},
                {"type": "gain", "db": -6.0}
            ]
        });
        let mut chain = chain_from_value(&v, 44100, 1);
        let mut samples = vec![0.5, 0.5, 0.5, 0.5];
        chain.process(&mut samples, 1, 44100);
        // +6 dB then -6 dB should ≈ unity
        for s in &samples {
            assert!((s - 0.5).abs() < 0.01);
        }
    }
}
