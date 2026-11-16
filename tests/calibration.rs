//! Deterministic synthetic calibration tests (known delay, gain, noise, missing, false peaks,
//! sample-rate mismatch, clipping).

use duetsync::audio::resample::resample;
use duetsync::calibration::stats::Sample;
use duetsync::calibration::*;

const SR: u32 = 48_000;

/// Simple deterministic LCG noise in [-1, 1].
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

struct Scenario {
    delay_ms: f64,
    gain: f32,
    noise: f32,
    missing: Vec<usize>,
    distractor: Option<(f64, f32)>, // (extra delay ms, relative gain)
    clip: bool,
    rate_ratio: f64, // >1: capture clock runs fast
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            delay_ms: 137.4,
            gain: 0.6,
            noise: 0.01,
            missing: vec![],
            distractor: None,
            clip: false,
            rate_ratio: 1.0,
        }
    }
}

fn run(s: &Scenario) -> CalibrationResult {
    let plan = CalibrationPlan::new(1_000_000, 8, 1500.0);
    let tpl = plan.chirp.generate(SR);
    let len = ((plan.end_host_us() - 1_000_000) as f64 / 1e6 * SR as f64) as usize + SR as usize;
    let mut sig = vec![0f32; len];
    let rec_start = 1_000_000 - 200_000; // recording starts 200 ms before first chirp
    let add = |at_us: f64, g: f32, sig: &mut Vec<f32>| {
        let pos = (at_us - rec_start as f64) * SR as f64 / 1e6;
        let i0 = pos.floor() as usize;
        let fr = (pos - pos.floor()) as f32;
        for (j, &v) in tpl.iter().enumerate() {
            // fractional delay by linear interpolation (good enough at this bandwidth)
            if i0 + j + 1 < sig.len() {
                sig[i0 + j] += v * g * (1.0 - fr);
                sig[i0 + j + 1] += v * g * fr;
            }
        }
    };
    for (k, &t) in plan.times_host_us.iter().enumerate() {
        if s.missing.contains(&k) {
            continue;
        }
        let at = t as f64 + s.delay_ms * 1000.0;
        add(at, s.gain, &mut sig);
        if let Some((d, g)) = s.distractor {
            add(at + d * 1000.0, s.gain * g, &mut sig);
        }
    }
    let mut rng = Lcg(42);
    for v in sig.iter_mut() {
        *v += rng.next() * s.noise;
        if s.clip {
            *v = (*v * 4.0).clamp(-1.0, 1.0);
        }
    }
    if s.rate_ratio != 1.0 {
        // Device sample clock differs from nominal; we treat captured data as nominal SR.
        sig = resample(&sig, 1, SR as f64, SR as f64 * s.rate_ratio);
    }
    let rec = Recording {
        samples: sig,
        start_us: rec_start,
        sample_rate: SR,
        rate_hz: SR as f64,
    };
    let refs: Vec<Option<i64>> = plan.times_host_us.iter().map(|&t| Some(t)).collect();
    let found = locate_chirps(
        &rec,
        &refs,
        &plan.chirp,
        plan.max_latency_ms,
        &DetectorParams::default(),
    );
    let per = found
        .into_iter()
        .zip(&refs)
        .map(|(r, rf)| {
            r.map(|(t, d)| Sample {
                latency_ms: (t - rf.unwrap()) as f64 / 1000.0,
                ncc: d.ncc,
                clipped: d.clipped_samples > 0,
            })
        })
        .collect();
    analyze(per, 0.5, 4)
}

fn assert_close(r: &CalibrationResult, want: f64, tol: f64) {
    let m = r
        .median_ms
        .unwrap_or_else(|| panic!("no median: {:?}", r.messages));
    assert!(
        (m - want).abs() <= tol,
        "median {m:.3} vs {want} ({:?})",
        r.messages
    );
}

#[test]
fn known_delays_within_5ms() {
    for d in [0.0, 12.3, 80.0, 137.4, 333.3, 777.7] {
        let r = run(&Scenario {
            delay_ms: d,
            ..Default::default()
        });
        assert_eq!(r.status, CalibrationStatus::Valid, "{d}: {:?}", r.messages);
        assert_close(&r, d, 0.5);
        assert_eq!(r.accepted, 8);
    }
}

#[test]
fn gain_levels() {
    for g in [0.02, 0.1, 0.9] {
        let r = run(&Scenario {
            gain: g,
            ..Default::default()
        });
        assert_close(&r, 137.4, 1.0);
    }
}

#[test]
fn heavy_noise_still_detected_or_rejected_honestly() {
    // SNR ≈ -6 dB per sample; correlation gain still finds it.
    let r = run(&Scenario {
        gain: 0.1,
        noise: 0.3,
        ..Default::default()
    });
    if r.median_ms.is_some() {
        assert_close(&r, 137.4, 5.0);
    }
    // Pure noise: must be rejected, never a fake value.
    let r = run(&Scenario {
        gain: 0.0,
        noise: 0.3,
        ..Default::default()
    });
    assert_eq!(r.status, CalibrationStatus::Invalid);
    assert!(r.median_ms.is_none());
}

#[test]
fn missing_signals() {
    let r = run(&Scenario {
        missing: vec![1, 4, 6],
        ..Default::default()
    });
    assert_eq!(r.accepted, 5);
    assert_close(&r, 137.4, 0.5);
    assert!(r.messages.iter().any(|m| m.contains("not detected")));

    let r = run(&Scenario {
        missing: (0..8).collect(),
        ..Default::default()
    });
    assert_eq!(r.status, CalibrationStatus::Invalid);
    assert!(r.median_ms.is_none());
}

#[test]
fn false_peaks() {
    // Weaker echo 60 ms later: still locks onto the direct path.
    let r = run(&Scenario {
        distractor: Some((60.0, 0.5)),
        ..Default::default()
    });
    assert_close(&r, 137.4, 0.5);
    // Equal-strength duplicate: ambiguous, must be rejected.
    let r = run(&Scenario {
        distractor: Some((60.0, 1.0)),
        ..Default::default()
    });
    assert_eq!(r.status, CalibrationStatus::Invalid, "{:?}", r.median_ms);
}

#[test]
fn clipping_detected() {
    let r = run(&Scenario {
        clip: true,
        gain: 0.9,
        ..Default::default()
    });
    assert!(r.clipped > 0);
    assert_ne!(r.status, CalibrationStatus::Valid);
}

#[test]
fn sample_rate_mismatch() {
    // 100 ppm capture clock error accumulates ~1 ms over ~12 s; within the 5 ms target.
    let r = run(&Scenario {
        rate_ratio: 1.0001,
        ..Default::default()
    });
    assert_close(&r, 137.4, 5.0);
    // 44.1 kHz capture resampled to 48 kHz internal.
    let plan = CalibrationPlan::new(0, 1, 1500.0);
    let tpl = plan.chirp.generate(44_100);
    let mut x = vec![0f32; 44_100];
    let off = 9_000;
    x[off..off + tpl.len()].copy_from_slice(&tpl);
    let y = resample(&x, 1, 44_100.0, 48_000.0);
    let d = detect_in_window(&y, &plan.chirp.generate(48_000), &DetectorParams::default())
        .expect("detect after resampling");
    let want = off as f64 * 48_000.0 / 44_100.0;
    assert!(
        (d.position - want).abs() < 48.0 * 0.5,
        "{} vs {want}",
        d.position
    );
}
