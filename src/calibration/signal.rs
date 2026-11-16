//! Band-limited linear chirp test signal (survives voice codecs better than clicks).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChirpSpec {
    pub f0_hz: f64,
    pub f1_hz: f64,
    pub duration_ms: f64,
    pub amplitude: f32,
}

impl Default for ChirpSpec {
    fn default() -> Self {
        Self {
            f0_hz: 500.0,
            f1_hz: 3000.0,
            duration_ms: 250.0,
            amplitude: 0.5,
        }
    }
}

impl ChirpSpec {
    /// Mono samples with a 10 % Tukey taper on each end.
    pub fn generate(&self, sample_rate: u32) -> Vec<f32> {
        let sr = sample_rate as f64;
        let n = (self.duration_ms * sr / 1000.0).round() as usize;
        let t_total = n as f64 / sr;
        let k = (self.f1_hz - self.f0_hz) / t_total;
        let taper = (n as f64 * 0.1).max(1.0);
        (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                let phase = 2.0 * std::f64::consts::PI * (self.f0_hz * t + 0.5 * k * t * t);
                let edge = (i as f64).min((n - 1 - i) as f64);
                let w = if edge < taper {
                    0.5 - 0.5 * (std::f64::consts::PI * edge / taper).cos()
                } else {
                    1.0
                };
                (phase.sin() * w) as f32 * self.amplitude
            })
            .collect()
    }
}
