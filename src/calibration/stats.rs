//! Robust statistics over repeated chirp measurements.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CalibrationStatus {
    Valid,
    /// Measured, but spread or clock uncertainty is high.
    LowConfidence,
    Invalid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibrationResult {
    pub status: CalibrationStatus,
    pub median_ms: Option<f64>,
    /// Robust standard deviation (1.4826·MAD) of accepted repetitions.
    pub jitter_ms: Option<f64>,
    pub min_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub accepted: usize,
    pub total: usize,
    pub outliers: usize,
    pub clipped: usize,
    pub clock_uncertainty_ms: f64,
    /// ≈95 % bound on the median error, including clock uncertainty.
    pub uncertainty_ms: Option<f64>,
    /// 0..1 heuristic: detection quality × repetition yield × stability.
    pub confidence: f64,
    pub mean_ncc: f64,
    pub per_chirp: Vec<Result<f64, String>>,
    pub messages: Vec<String>,
}

pub struct Sample {
    pub latency_ms: f64,
    pub ncc: f32,
    pub clipped: bool,
}

pub fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Combine per-chirp outcomes into a result. Never fabricates a value: with fewer than
/// `min_valid` accepted repetitions the status is `Invalid` and `median_ms` is `None`.
pub fn analyze(
    per: Vec<Result<Sample, String>>,
    clock_uncertainty_ms: f64,
    min_valid: usize,
) -> CalibrationResult {
    let total = per.len();
    let mut messages = Vec::new();
    let clipped = per
        .iter()
        .filter(|r| matches!(r, Ok(s) if s.clipped))
        .count();
    if clipped > 0 {
        messages.push(format!(
            "{clipped} detection(s) clipped — lower the calibration/Discord volume"
        ));
    }
    let ok: Vec<&Sample> = per.iter().filter_map(|r| r.as_ref().ok()).collect();
    let missing = total - ok.len();
    if missing > 0 {
        messages.push(format!("{missing} of {total} test signals not detected"));
    }
    let per_chirp: Vec<Result<f64, String>> = per
        .iter()
        .map(|r| match r {
            Ok(s) => Ok(s.latency_ms),
            Err(e) => Err(e.clone()),
        })
        .collect();

    let mut result = CalibrationResult {
        status: CalibrationStatus::Invalid,
        median_ms: None,
        jitter_ms: None,
        min_ms: None,
        max_ms: None,
        accepted: 0,
        total,
        outliers: 0,
        clipped,
        clock_uncertainty_ms,
        uncertainty_ms: None,
        confidence: 0.0,
        mean_ncc: 0.0,
        per_chirp,
        messages,
    };
    if ok.len() < min_valid {
        result.messages.push(format!(
            "need at least {min_valid} valid detections; measurement rejected"
        ));
        return result;
    }

    let mut vals: Vec<f64> = ok.iter().map(|s| s.latency_ms).collect();
    let med0 = median(&mut vals.clone());
    let mut dev: Vec<f64> = vals.iter().map(|v| (v - med0).abs()).collect();
    let sd0 = 1.4826 * median(&mut dev);
    let gate = (3.0 * sd0).max(5.0);
    let accepted: Vec<&Sample> = ok
        .iter()
        .copied()
        .filter(|s| (s.latency_ms - med0).abs() <= gate)
        .collect();
    result.outliers = ok.len() - accepted.len();
    if result.outliers > 0 {
        result.messages.push(format!(
            "{} outlier repetition(s) discarded",
            result.outliers
        ));
    }
    if accepted.len() < min_valid {
        result
            .messages
            .push("repetitions disagree too much; measurement rejected".into());
        return result;
    }
    vals = accepted.iter().map(|s| s.latency_ms).collect();
    let med = median(&mut vals);
    let mut dev: Vec<f64> = vals.iter().map(|v| (v - med).abs()).collect();
    let sd = (1.4826 * median(&mut dev)).max(0.0);
    let n = accepted.len() as f64;
    let mean_ncc = accepted.iter().map(|s| s.ncc as f64).sum::<f64>() / n;
    // Std. error of a median ≈ 1.2533·σ/√n; 2× for ~95 %, plus a 0.1 ms sub-sample floor.
    let unc = 2.0 * 1.2533 * sd / n.sqrt() + clock_uncertainty_ms + 0.1;
    let yield_f = n / total as f64;
    let stability = 1.0 / (1.0 + (sd / 10.0).powi(2));
    let conf = (mean_ncc.clamp(0.0, 1.0).sqrt() * yield_f * stability).clamp(0.0, 1.0);

    result.accepted = accepted.len();
    result.median_ms = Some(med);
    result.jitter_ms = Some(sd);
    result.min_ms = vals.iter().copied().reduce(f64::min);
    result.max_ms = vals.iter().copied().reduce(f64::max);
    result.uncertainty_ms = Some(unc);
    result.mean_ncc = mean_ncc;
    result.confidence = conf;
    result.status = if unc <= 20.0 && conf >= 0.4 && clipped == 0 {
        CalibrationStatus::Valid
    } else {
        result
            .messages
            .push("measurement usable but low confidence — consider recalibrating".into());
        CalibrationStatus::LowConfidence
    };
    result
}
