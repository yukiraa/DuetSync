//! Correlation and conservative smoothing for live delay estimates.

use super::from_u8;
use std::collections::VecDeque;

const HISTORY_MS: i64 = 12_000;
const WINDOW_MS: usize = 4_000;
const LOOKBACK_MS: i64 = 1_300;

#[derive(Clone, Copy, Debug)]
pub struct LiveEstimate {
    pub latency_ms: f64,
    pub ncc: f32,
    pub voiced: f32,
}

#[derive(Default)]
struct Series {
    start_ms: Option<i64>,
    values: Vec<f32>, // NaN means no packet covered this millisecond.
}

impl Series {
    fn push(&mut self, start_ms: i64, values: &[f32]) {
        if values.is_empty() {
            return;
        }
        let end_ms = start_ms + values.len() as i64;
        if self.start_ms.is_none() {
            self.start_ms = Some(start_ms);
        }
        let old_start = self.start_ms.unwrap_or(start_ms);
        if start_ms < old_start {
            let add = (old_start - start_ms) as usize;
            let mut prefix = vec![f32::NAN; add];
            prefix.extend_from_slice(&self.values);
            self.values = prefix;
            self.start_ms = Some(start_ms);
        }
        let base = self.start_ms.unwrap_or(start_ms);
        let wanted = (end_ms - base) as usize;
        if self.values.len() < wanted {
            self.values.resize(wanted, f32::NAN);
        }
        let pos = (start_ms - base) as usize;
        self.values[pos..pos + values.len()].copy_from_slice(values);
    }

    fn prune(&mut self, newest_ms: i64) {
        let Some(start) = self.start_ms else { return };
        let keep_from = newest_ms - HISTORY_MS;
        if keep_from <= start {
            return;
        }
        let drop_n = ((keep_from - start) as usize).min(self.values.len());
        self.values.drain(..drop_n);
        self.start_ms = Some(start + drop_n as i64);
    }

    fn window(&self, start_ms: i64, len: usize) -> Option<&[f32]> {
        let base = self.start_ms?;
        let offset = start_ms.checked_sub(base)?;
        if offset < 0 {
            return None;
        }
        let offset = offset as usize;
        let out = self.values.get(offset..offset.checked_add(len)?)?;
        out.iter().all(|v| v.is_finite()).then_some(out)
    }
}

/// Keeps timestamped envelope bins in the host clock domain.
#[derive(Default)]
pub struct LiveTracker {
    guest: Series,
    host: Series,
}

impl LiveTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_guest(&mut self, start_host_ms: i64, values: &[u8]) {
        let decoded = from_u8(values);
        self.guest.push(start_host_ms, &decoded);
        self.guest.prune(start_host_ms + decoded.len() as i64);
    }

    pub fn push_host(&mut self, start_ms: i64, values: &[f32]) {
        self.host.push(start_ms, values);
        self.host.prune(start_ms + values.len() as i64);
    }

    pub fn estimate(&self, now_ms: i64) -> Option<LiveEstimate> {
        let guest_end = now_ms - LOOKBACK_MS;
        let guest_start = guest_end - WINDOW_MS as i64;
        let guest = self.guest.window(guest_start, WINDOW_MS)?;
        let gmean = guest.iter().map(|&v| v as f64).sum::<f64>() / WINDOW_MS as f64;
        let gvar = guest
            .iter()
            .map(|&v| {
                let d = v as f64 - gmean;
                d * d
            })
            .sum::<f64>();
        // -50 dBFS separates actual voice energy from transport-floor silence.
        let voiced =
            guest.iter().filter(|&&v| v > 0.003_162_277_6).count() as f32 / WINDOW_MS as f32;
        if voiced < 0.2 || gvar < 1e-8 {
            return None;
        }

        let mut scores = [0.0f32; 1001];
        for (lag, score) in scores.iter_mut().enumerate() {
            let host = self.host.window(guest_start + lag as i64, WINDOW_MS)?;
            let hmean = host.iter().map(|&v| v as f64).sum::<f64>() / WINDOW_MS as f64;
            let (mut dot, mut hvar) = (0.0, 0.0);
            for (&g, &h) in guest.iter().zip(host) {
                let gd = g as f64 - gmean;
                let hd = h as f64 - hmean;
                dot += gd * hd;
                hvar += hd * hd;
            }
            *score = if hvar > 1e-12 {
                (dot / (gvar * hvar).sqrt()) as f32
            } else {
                -1.0
            };
        }
        let (peak_idx, &peak) = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))?;
        if peak < 0.45 || peak_idx == 0 || peak_idx == 1000 {
            return None;
        }
        let rival = scores
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(peak_idx) > 40)
            .map(|(_, &v)| v)
            .fold(-1.0f32, f32::max);
        if rival > 0.0 && peak / rival < 1.15 {
            return None;
        }
        let left = scores[peak_idx - 1] as f64;
        let center = peak as f64;
        let right = scores[peak_idx + 1] as f64;
        let den = left - 2.0 * center + right;
        let frac = if den.abs() > 1e-12 {
            (0.5 * (left - right) / den).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Some(LiveEstimate {
            latency_ms: peak_idx as f64 + frac,
            ncc: peak,
            voiced,
        })
    }
}

/// Median smoothing and deliberately slow automatic OBS re-application.
pub struct LiveFilter {
    recent: VecDeque<f64>,
    unseen: u64,
    checked: u64,
    consecutive: u8,
    last_reapply_ms: Option<i64>,
}

impl Default for LiveFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveFilter {
    pub fn new() -> Self {
        Self {
            recent: VecDeque::with_capacity(5),
            unseen: 0,
            checked: 0,
            consecutive: 0,
            last_reapply_ms: None,
        }
    }

    pub fn push(&mut self, estimate: LiveEstimate) {
        // A sustained step must not be hidden behind five stale values: begin
        // a fresh candidate median.  It still needs a second accepted sample
        // before `should_reapply` permits a change.
        if self
            .median()
            .is_some_and(|median| (estimate.latency_ms - median).abs() > 30.0)
        {
            self.recent.clear();
        }
        if self.recent.len() == 5 {
            self.recent.pop_front();
        }
        self.recent.push_back(estimate.latency_ms);
        self.unseen += 1;
    }

    pub fn median(&self) -> Option<f64> {
        if self.recent.is_empty() {
            return None;
        }
        let mut values: Vec<f64> = self.recent.iter().copied().collect();
        values.sort_by(f64::total_cmp);
        Some(values[values.len() / 2])
    }

    pub fn should_reapply(&mut self, applied_ms: f64, now_ms: i64) -> Option<f64> {
        if self.checked == self.unseen {
            return None;
        }
        self.checked = self.unseen;
        let median = self.median()?;
        if (median - applied_ms).abs() > 30.0 {
            self.consecutive = self.consecutive.saturating_add(1);
        } else {
            self.consecutive = 0;
        }
        let enough_time = self
            .last_reapply_ms
            .is_none_or(|last| now_ms - last >= 10_000);
        if self.consecutive >= 2 && enough_time {
            self.consecutive = 0;
            self.last_reapply_ms = Some(now_ms);
            Some(median)
        } else {
            None
        }
    }
}
