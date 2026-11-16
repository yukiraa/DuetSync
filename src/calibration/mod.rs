//! Latency calibration: test signal, normalized cross-correlation detector, statistics.

pub mod detect;
pub mod signal;
pub mod stats;

pub use detect::{detect_in_window, Detection, DetectorParams};
pub use signal::ChirpSpec;
pub use stats::{analyze, CalibrationResult, CalibrationStatus};

use serde::{Deserialize, Serialize};

/// Plan of one calibration run, shared host → guest.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CalibrationPlan {
    pub chirp: ChirpSpec,
    /// Scheduled presentation times of each chirp in HOST clock microseconds.
    pub times_host_us: Vec<i64>,
    /// Max latency searched after the reference time.
    pub max_latency_ms: f64,
}

impl CalibrationPlan {
    pub fn new(start_host_us: i64, count: usize, spacing_ms: f64) -> Self {
        let times = (0..count)
            .map(|k| start_host_us + (k as f64 * spacing_ms * 1000.0) as i64)
            .collect();
        Self {
            chirp: ChirpSpec::default(),
            times_host_us: times,
            max_latency_ms: (spacing_ms - 300.0).clamp(200.0, 1200.0),
        }
    }

    pub fn end_host_us(&self) -> i64 {
        self.times_host_us.last().copied().unwrap_or(0)
            + (self.chirp.duration_ms * 1000.0) as i64
            + (self.max_latency_ms * 1000.0) as i64
    }
}

/// A timestamped mono recording at `SAMPLE_RATE`.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub samples: Vec<f32>,
    /// Time (local clock µs) of `samples[0]`.
    pub start_us: i64,
    /// Nominal rate (template generation).
    pub sample_rate: u32,
    /// Effective rate measured from device timestamps (≈ sample_rate).
    pub rate_hz: f64,
}

impl Recording {
    pub fn time_of(&self, sample_pos: f64) -> i64 {
        self.start_us + (sample_pos * 1e6 / self.rate_hz).round() as i64
    }
    pub fn index_of(&self, t_us: i64) -> f64 {
        (t_us - self.start_us) as f64 * self.rate_hz / 1e6
    }
}

/// Find each chirp of `plan` in `rec`, searching `[ref - pre, ref + max_latency]` where
/// `ref_times` are in the same clock as `rec`. Returns per-chirp detected times (µs).
pub fn locate_chirps(
    rec: &Recording,
    ref_times_us: &[Option<i64>],
    chirp: &ChirpSpec,
    max_latency_ms: f64,
    params: &DetectorParams,
) -> Vec<Result<(i64, Detection), String>> {
    let tpl = chirp.generate(rec.sample_rate);
    ref_times_us
        .iter()
        .map(|r| {
            let r = r.ok_or_else(|| "no reference time".to_string())?;
            let lo = rec.index_of(r - 50_000).floor().max(0.0) as usize;
            let hi = (rec.index_of(r + (max_latency_ms * 1000.0) as i64).ceil() as usize
                + tpl.len())
            .min(rec.samples.len());
            if hi <= lo + tpl.len() {
                return Err("window outside recording".into());
            }
            let d = detect_in_window(&rec.samples[lo..hi], &tpl, params)?;
            let pos = lo as f64 + d.position;
            Ok((rec.time_of(pos), d))
        })
        .collect()
}
