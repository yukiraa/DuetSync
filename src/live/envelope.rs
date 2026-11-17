//! Timestamped 1 ms voice-energy envelopes.

use crate::audio::Anchor;

const RATE: f64 = 48_000.0;

#[derive(Clone, Copy)]
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
    fn lowpass(hz: f32) -> Self {
        Self::new(hz, false)
    }

    fn highpass(hz: f32) -> Self {
        Self::new(hz, true)
    }

    fn new(hz: f32, highpass: bool) -> Self {
        let w = 2.0 * std::f32::consts::PI * hz / RATE as f32;
        let c = w.cos();
        let alpha = w.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let (b0, b1, b2) = if highpass {
            ((1.0 + c) * 0.5, -(1.0 + c), (1.0 + c) * 0.5)
        } else {
            ((1.0 - c) * 0.5, 1.0 - c, (1.0 - c) * 0.5)
        };
        let a0 = 1.0 + alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn apply(&mut self, x: f32) -> f32 {
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

/// Builds 1 ms RMS bins on the capture clock.  `push` emits bins completed by
/// the supplied packets; the partially filled newest bin is held until later.
pub struct EnvelopeBuilder {
    high: Biquad,
    low: Biquad,
    current_ms: Option<i64>,
    sum_sq: f64,
    count: u32,
    ready_start_ms: Option<i64>,
    ready: Vec<f32>,
}

impl Default for EnvelopeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvelopeBuilder {
    pub fn new() -> Self {
        Self {
            high: Biquad::highpass(300.0),
            low: Biquad::lowpass(3400.0),
            current_ms: None,
            sum_sq: 0.0,
            count: 0,
            ready_start_ms: None,
            ready: Vec::with_capacity(256),
        }
    }

    fn emit(&mut self, ms: i64, value: f32) {
        match self.ready_start_ms {
            None => self.ready_start_ms = Some(ms),
            Some(start) if ms < start => return,
            Some(start) => {
                let wanted = (ms - start) as usize;
                if self.ready.len() < wanted {
                    self.ready.resize(wanted, 0.0);
                }
                if self.ready.len() > wanted {
                    self.ready[wanted] = value;
                    return;
                }
            }
        }
        self.ready.push(value);
    }

    fn finish_current(&mut self) {
        if let Some(ms) = self.current_ms {
            let rms = if self.count == 0 {
                0.0
            } else {
                (self.sum_sq / self.count as f64).sqrt() as f32
            };
            self.emit(ms, rms);
        }
        self.sum_sq = 0.0;
        self.count = 0;
    }

    /// Process capture packets. `samples` is the contiguous packet payload
    /// matching `anchors`, as returned by `CaptureHandle::drain`.
    pub fn push(&mut self, samples: &[f32], anchors: &[Anchor]) -> Option<(i64, Vec<f32>)> {
        let mut offset = 0usize;
        for anchor in anchors {
            let n = (anchor.frames as usize).min(samples.len().saturating_sub(offset));
            for (i, &sample) in samples[offset..offset + n].iter().enumerate() {
                let us = anchor.t_us + ((i as f64 * 1_000_000.0 / RATE).round() as i64);
                let ms = us.div_euclid(1000);
                if self.current_ms != Some(ms) {
                    self.finish_current();
                    self.current_ms = Some(ms);
                }
                let filtered = self.low.apply(self.high.apply(sample));
                self.sum_sq += (filtered as f64) * (filtered as f64);
                self.count += 1;
            }
            offset += n;
        }
        let start = self.ready_start_ms.take()?;
        Some((start, std::mem::take(&mut self.ready)))
    }

    /// Complete the current partial bin, useful when a stream is stopping.
    pub fn finish(&mut self) -> Option<(i64, Vec<f32>)> {
        self.finish_current();
        let start = self.ready_start_ms.take()?;
        Some((start, std::mem::take(&mut self.ready)))
    }
}

/// Logarithmic transport quantisation: -70 dBFS through 0 dBFS maps to 0..255.
pub fn to_u8(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .map(|&v| {
            let db = 20.0 * v.max(1e-12).log10();
            ((db.clamp(-70.0, 0.0) + 70.0) * (255.0 / 70.0)).round() as u8
        })
        .collect()
}

pub fn from_u8(values: &[u8]) -> Vec<f32> {
    values
        .iter()
        .map(|&v| 10.0f32.powf(((v as f32 * 70.0 / 255.0) - 70.0) / 20.0))
        .collect()
}
