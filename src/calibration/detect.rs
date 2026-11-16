//! Normalized cross-correlation peak detection with ambiguity and level checks.

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct DetectorParams {
    /// Minimum normalized correlation (0..1) for a valid peak.
    pub min_ncc: f32,
    /// Peak must exceed the best peak outside the exclusion zone by this factor.
    pub min_peak_ratio: f32,
    /// Exclusion zone around the main peak (ms) — covers main lobe + early reflections.
    pub exclusion_ms: f64,
    /// Minimum RMS of the matched segment (linear) to avoid fitting to silence.
    pub min_rms: f32,
    pub sample_rate: u32,
}

impl Default for DetectorParams {
    fn default() -> Self {
        Self {
            min_ncc: 0.30,
            min_peak_ratio: 1.4,
            exclusion_ms: 8.0,
            min_rms: 1e-3,
            sample_rate: crate::SAMPLE_RATE,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Detection {
    /// Sub-sample position of the template start inside the searched window.
    pub position: f64,
    pub ncc: f32,
    pub peak_ratio: f32,
    pub clipped_samples: usize,
    pub rms: f32,
}

/// Search `window` for `template`. Returns Err with a human reason when no reliable peak.
pub fn detect_in_window(
    window: &[f32],
    template: &[f32],
    p: &DetectorParams,
) -> Result<Detection, String> {
    let m = template.len();
    if m == 0 || window.len() < m {
        return Err("window shorter than template".into());
    }
    let lags = window.len() - m + 1;
    let corr = xcorr_fft(window, template, lags);

    // Sliding energy of the window via prefix sums (f64 for precision).
    let mut prefix = Vec::with_capacity(window.len() + 1);
    prefix.push(0.0f64);
    let mut acc = 0.0f64;
    for &x in window {
        acc += (x as f64) * (x as f64);
        prefix.push(acc);
    }
    let tpl_norm = template
        .iter()
        .map(|&x| (x as f64) * (x as f64))
        .sum::<f64>()
        .sqrt();
    let rms_floor = (p.min_rms as f64).powi(2) * m as f64;
    let ncc: Vec<f32> = (0..lags)
        .map(|k| {
            let e = prefix[k + m] - prefix[k];
            if e <= rms_floor {
                0.0
            } else {
                (corr[k] as f64 / (tpl_norm * e.sqrt())) as f32
            }
        })
        .collect();

    let (best, &best_v) = ncc
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .expect("non-empty");
    if best_v < p.min_ncc {
        return Err(format!(
            "signal not found (best correlation {best_v:.2} < {:.2})",
            p.min_ncc
        ));
    }
    let excl = (p.exclusion_ms * p.sample_rate as f64 / 1000.0) as usize;
    let second = ncc
        .iter()
        .enumerate()
        .filter(|(i, _)| i.abs_diff(best) > excl)
        .map(|(_, v)| *v)
        .fold(0.0f32, f32::max);
    let ratio = if second <= 1e-6 {
        f32::INFINITY
    } else {
        best_v / second
    };
    if ratio < p.min_peak_ratio {
        return Err(format!(
            "ambiguous correlation (peak ratio {ratio:.2} < {:.2})",
            p.min_peak_ratio
        ));
    }
    // Parabolic interpolation on raw correlation for sub-sample timing.
    let frac = if best > 0 && best + 1 < lags {
        let (a, b, c) = (corr[best - 1], corr[best], corr[best + 1]);
        let den = a - 2.0 * b + c;
        if den.abs() > 1e-12 {
            (0.5 * (a - c) / den).clamp(-0.5, 0.5) as f64
        } else {
            0.0
        }
    } else {
        0.0
    };
    let seg = &window[best..best + m];
    let clipped = seg.iter().filter(|x| x.abs() >= 0.999).count();
    let rms = ((prefix[best + m] - prefix[best]) / m as f64).sqrt() as f32;
    Ok(Detection {
        position: best as f64 + frac,
        ncc: best_v,
        peak_ratio: ratio,
        clipped_samples: clipped,
        rms,
    })
}

/// corr[k] = Σ_j x[k+j]·t[j] for k in 0..lags, via FFT.
fn xcorr_fft(x: &[f32], t: &[f32], lags: usize) -> Vec<f32> {
    let n = (x.len() + t.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut a: Vec<Complex32> = x.iter().map(|&v| Complex32::new(v, 0.0)).collect();
    a.resize(n, Complex32::default());
    let mut b: Vec<Complex32> = t.iter().map(|&v| Complex32::new(v, 0.0)).collect();
    b.resize(n, Complex32::default());
    fwd.process(&mut a);
    fwd.process(&mut b);
    for (u, v) in a.iter_mut().zip(&b) {
        *u *= v.conj();
    }
    inv.process(&mut a);
    let scale = 1.0 / n as f32;
    a[..lags].iter().map(|c| c.re * scale).collect()
}
