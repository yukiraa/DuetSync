//! Audio I/O. WASAPI (event-driven, shared mode with OS rate conversion) on Windows.

pub mod resample;

#[cfg(windows)]
pub mod process;
#[cfg(windows)]
pub mod wasapi;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
}

/// Where a capture stream gets its audio from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureSource {
    /// Microphone / input endpoint (`None` = system default).
    Input(Option<String>),
    /// Loopback of an output endpoint ("what you hear"; `None` = default output).
    OutputLoopback(Option<String>),
    /// Process-tree loopback (Windows 10 2004+), e.g. Discord's root process.
    Process { pid: u32, name: String },
}

impl CaptureSource {
    pub fn label(&self) -> String {
        match self {
            Self::Input(None) => "Default input".into(),
            Self::Input(Some(id)) => format!("Input {id}"),
            Self::OutputLoopback(None) => "Default output (loopback)".into(),
            Self::OutputLoopback(Some(id)) => format!("Output loopback {id}"),
            Self::Process { pid, name } => format!("{name} (pid {pid}) process loopback"),
        }
    }
}

/// Mono capture timestamp anchor: `frame` (stream frame counter) was captured at `t_us`.
#[derive(Clone, Copy, Default, Debug)]
pub struct Anchor {
    pub frame: u64,
    pub t_us: i64,
    pub frames: u32,
}

/// Least-squares fit `t = a + b·(frame - f0)` over anchors. Returns (a, b µs/frame).
pub fn fit_anchors(anchors: &[Anchor], nominal_rate: f64) -> Option<(f64, f64)> {
    let first = anchors.first()?;
    let nominal = 1e6 / nominal_rate;
    if anchors.len() < 3 {
        return Some((first.t_us as f64, nominal));
    }
    let n = anchors.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for a in anchors {
        let x = (a.frame - first.frame) as f64;
        let y = (a.t_us - first.t_us) as f64;
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let den = n * sxx - sx * sx;
    if den.abs() < 1e-9 {
        return Some((first.t_us as f64, nominal));
    }
    let b = (n * sxy - sx * sy) / den;
    // Reject implausible timestamp slopes (>1000 ppm) and fall back to nominal rate.
    if ((b - nominal) / nominal).abs() > 1e-3 {
        let a = anchors
            .iter()
            .map(|a| a.t_us as f64 - nominal * (a.frame - first.frame) as f64)
            .sum::<f64>()
            / n;
        return Some((a, nominal));
    }
    let a = (sy - b * sx) / n + first.t_us as f64;
    Some((a, b))
}

/// Place captured packets on the timeline using their timestamps. Loopback and virtual
/// devices deliver *no packets* while silent, so concatenating samples would lose time.
/// Gaps larger than 4 ms are filled with silence; smaller deviations (timestamp jitter)
/// are ignored. Returns (samples, time of samples[0]).
pub fn regrid(raw: &[f32], anchors: &[Anchor], rate: f64) -> Option<(Vec<f32>, i64)> {
    let first = anchors.first()?;
    let t0 = first.t_us;
    let tol = (rate * 0.004) as i64;
    let mut out: Vec<f32> = Vec::with_capacity(raw.len() + raw.len() / 4);
    let mut cursor = 0usize;
    for a in anchors {
        let n = (a.frames as usize).min(raw.len().saturating_sub(cursor));
        let target = ((a.t_us - t0) as f64 * rate / 1e6).round() as i64;
        let gap = target - out.len() as i64;
        if gap > tol {
            out.resize(target as usize, 0.0);
        } else if gap < -tol {
            // Packet overlaps what we already have (timestamp went backwards): overwrite.
            let start = target.max(0) as usize;
            out.truncate(start);
        }
        out.extend_from_slice(&raw[cursor..cursor + n]);
        cursor += n;
    }
    Some((out, t0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regrid_fills_silent_gaps() {
        let rate = 48_000.0;
        // Two 480-frame packets, then a 1 s gap with no packets, then another packet.
        let raw: Vec<f32> = (0..1440).map(|i| i as f32).collect();
        let anchors = [
            Anchor {
                frame: 0,
                t_us: 1_000_000,
                frames: 480,
            },
            Anchor {
                frame: 480,
                t_us: 1_010_000,
                frames: 480,
            },
            Anchor {
                frame: 960,
                t_us: 2_020_000,
                frames: 480,
            },
        ];
        let (out, t0) = regrid(&raw, &anchors, rate).unwrap();
        assert_eq!(t0, 1_000_000);
        assert_eq!(out.len(), 48_960 + 480);
        assert_eq!(out[959], 959.0);
        assert_eq!(out[30_000], 0.0);
        assert_eq!(out[48_960], 960.0);
        // Small jitter is not treated as a gap.
        let jit = [
            Anchor {
                frame: 0,
                t_us: 0,
                frames: 480,
            },
            Anchor {
                frame: 480,
                t_us: 10_100,
                frames: 480,
            },
        ];
        assert_eq!(regrid(&raw[..960], &jit, rate).unwrap().0.len(), 960);
    }

    #[test]
    fn anchor_fit_recovers_rate_and_offset() {
        let rate = 48_000.0 * 1.00005; // 50 ppm fast device
        let anchors: Vec<Anchor> = (0..200)
            .map(|k| {
                let frame = 1000 + k * 480;
                let jitter = if k % 2 == 0 { 30 } else { -30 };
                Anchor {
                    frame,
                    t_us: 5_000_000 + ((k * 480) as f64 * 1e6 / rate) as i64 + jitter,
                    frames: 480,
                }
            })
            .collect();
        let (a, b) = fit_anchors(&anchors, 48_000.0).unwrap();
        assert!((a - 5_000_000.0).abs() < 20.0, "{a}");
        assert!((b - 1e6 / rate).abs() < 1e-4, "{b}");
    }
}
