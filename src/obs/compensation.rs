//! Compensation math. Pure and unit-tested.
//!
//!   target      = max(latency_i) + margin
//!   offset_i    = baseline_i + (target - latency_i)
//!
//! `baseline_i` is the input's offset before DuetSync touched it. It is captured once and
//! reused, so recalibrating never stacks compensation on top of earlier compensation.

use super::client::{OFFSET_MAX_MS, OFFSET_MIN_MS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SourceLatency {
    pub input: String,
    /// Measured (or user-entered) latency of this source relative to real time, ms.
    pub latency_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OffsetChange {
    pub input: String,
    pub baseline_ms: i64,
    pub current_ms: i64,
    pub target_ms: i64,
    pub compensation_ms: i64,
}

/// Persistent memory of what DuetSync did to each input.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OffsetLedger {
    /// Offset before DuetSync's first change.
    pub baseline: BTreeMap<String, i64>,
    /// Last value DuetSync wrote.
    pub applied: BTreeMap<String, i64>,
}

pub fn compensation(sources: &[SourceLatency], margin_ms: f64) -> Vec<(String, f64)> {
    let target = sources
        .iter()
        .map(|s| s.latency_ms)
        .fold(f64::NEG_INFINITY, f64::max)
        + margin_ms;
    sources
        .iter()
        .map(|s| (s.input.clone(), target - s.latency_ms))
        .collect()
}

/// Decide new offsets. `current` = offsets read from OBS right now.
/// Returns changes plus notes (e.g. an offset was edited by the user outside DuetSync).
pub fn plan(
    sources: &[SourceLatency],
    margin_ms: f64,
    current: &BTreeMap<String, i64>,
    ledger: &OffsetLedger,
) -> Result<(Vec<OffsetChange>, Vec<String>), String> {
    if sources.is_empty() {
        return Err("no OBS sources selected for compensation".into());
    }
    let mut names: Vec<&str> = sources.iter().map(|s| s.input.as_str()).collect();
    names.sort_unstable();
    if names.windows(2).any(|w| w[0] == w[1]) {
        return Err("the same OBS input is assigned to more than one role".into());
    }
    let mut notes = Vec::new();
    let mut out = Vec::new();
    for (input, comp) in compensation(sources, margin_ms) {
        let cur = *current
            .get(&input)
            .ok_or_else(|| format!("OBS input '{input}' not found"))?;
        let baseline = match (ledger.baseline.get(&input), ledger.applied.get(&input)) {
            (Some(&b), Some(&a)) if a == cur => b,
            (Some(_), _) => {
                notes.push(format!(
                    "'{input}' was changed outside DuetSync ({cur} ms); using it as the new baseline"
                ));
                cur
            }
            (None, _) => cur,
        };
        let comp = comp.round() as i64;
        let target = baseline + comp;
        if !(OFFSET_MIN_MS..=OFFSET_MAX_MS).contains(&target) {
            return Err(format!(
                "'{input}' would need {target} ms, outside OBS's {OFFSET_MIN_MS}..{OFFSET_MAX_MS} ms range"
            ));
        }
        out.push(OffsetChange {
            input,
            baseline_ms: baseline,
            current_ms: cur,
            target_ms: target,
            compensation_ms: comp,
        });
    }
    Ok((out, notes))
}

impl OffsetLedger {
    pub fn record(&mut self, c: &OffsetChange) {
        self.baseline.insert(c.input.clone(), c.baseline_ms);
        self.applied.insert(c.input.clone(), c.target_ms);
    }
    pub fn forget(&mut self, input: &str) {
        self.baseline.remove(input);
        self.applied.remove(input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(v: &[(&str, f64)]) -> Vec<SourceLatency> {
        v.iter()
            .map(|(n, l)| SourceLatency {
                input: n.to_string(),
                latency_ms: *l,
            })
            .collect()
    }
    fn cur(v: &[(&str, i64)]) -> BTreeMap<String, i64> {
        v.iter().map(|(n, o)| (n.to_string(), *o)).collect()
    }

    #[test]
    fn formula() {
        let c = compensation(
            &src(&[("mic", 0.0), ("music", 0.0), ("guest", 180.0)]),
            20.0,
        );
        assert_eq!(c[0].1, 200.0);
        assert_eq!(c[1].1, 200.0);
        assert_eq!(c[2].1, 20.0);
    }

    #[test]
    fn repeated_calibration_does_not_stack() {
        let s = src(&[("mic", 0.0), ("guest", 150.0)]);
        let mut ledger = OffsetLedger::default();
        let mut obs = cur(&[("mic", 30), ("guest", 0)]); // user had 30 ms on mic already
        for _ in 0..3 {
            let (changes, notes) = plan(&s, 10.0, &obs, &ledger).unwrap();
            assert!(notes.is_empty());
            for c in &changes {
                ledger.record(c);
                obs.insert(c.input.clone(), c.target_ms);
            }
            assert_eq!(obs["mic"], 30 + 160);
            assert_eq!(obs["guest"], 10);
        }
        // New measurement replaces, never accumulates.
        let s2 = src(&[("mic", 0.0), ("guest", 100.0)]);
        let (changes, _) = plan(&s2, 10.0, &obs, &ledger).unwrap();
        assert_eq!(changes[0].target_ms, 30 + 110);
        // Restoring uses the original baseline.
        assert_eq!(ledger.baseline["mic"], 30);
        assert_eq!(ledger.baseline["guest"], 0);
    }

    #[test]
    fn external_edit_becomes_baseline() {
        let s = src(&[("mic", 0.0), ("guest", 100.0)]);
        let mut ledger = OffsetLedger::default();
        let obs = cur(&[("mic", 0), ("guest", 0)]);
        let (c, _) = plan(&s, 0.0, &obs, &ledger).unwrap();
        c.iter().for_each(|c| ledger.record(c));
        let edited = cur(&[("mic", 500), ("guest", 0)]);
        let (c, notes) = plan(&s, 0.0, &edited, &ledger).unwrap();
        assert_eq!(notes.len(), 1, "only the edited mic is flagged: {notes:?}");
        assert_eq!(c[0].baseline_ms, 500);
    }

    #[test]
    fn recording_before_a_failed_write_keeps_original_baseline() {
        let s = src(&[("mic", 0.0), ("guest", 150.0)]);
        let mut ledger = OffsetLedger::default();
        let obs = cur(&[("mic", 30), ("guest", 5)]);
        let (changes, _) = plan(&s, 10.0, &obs, &ledger).unwrap();
        // Ledger is recorded before the write; the write then fails, so OBS is unchanged.
        changes.iter().for_each(|c| ledger.record(c));
        assert_eq!(ledger.baseline["mic"], 30);
        let (again, notes) = plan(&s, 10.0, &obs, &ledger).unwrap();
        for (a, c) in again.iter().zip(&changes) {
            assert_eq!(a.baseline_ms, c.baseline_ms, "baseline stays the original");
            assert_eq!(a.target_ms, c.target_ms, "no stacked compensation");
        }
        assert_eq!(again[0].baseline_ms, 30);
        assert_eq!(again[1].baseline_ms, 5);
        assert_eq!(notes.len(), 2, "flagged as changed, not stacked: {notes:?}");
    }

    #[test]
    fn range_and_duplicates() {
        let s = src(&[("mic", 0.0), ("guest", 25_000.0)]);
        assert!(plan(
            &s,
            0.0,
            &cur(&[("mic", 0), ("guest", 0)]),
            &OffsetLedger::default()
        )
        .is_err());
        let d = src(&[("mic", 0.0), ("mic", 10.0)]);
        assert!(plan(&d, 0.0, &cur(&[("mic", 0)]), &OffsetLedger::default()).is_err());
        assert!(plan(
            &src(&[("x", 0.0)]),
            0.0,
            &cur(&[]),
            &OffsetLedger::default()
        )
        .is_err());
    }
}
