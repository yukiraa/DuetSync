//! Deterministic live-envelope and delay-tracker tests; no audio devices.

use duetsync::audio::Anchor;
use duetsync::live::{to_u8, EnvelopeBuilder, LiveEstimate, LiveFilter, LiveTracker};

const RATE: usize = 48_000;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    }
}

fn singing(seconds: usize) -> Vec<f32> {
    let mut rng = Lcg(0x5eed);
    let mut out = vec![0.0; seconds * RATE];
    let mut at = 0usize;
    let mut smooth = 0.0;
    while at < out.len() {
        // 3..6 Hz syllables, deterministically 70% voiced.
        let syllable = (RATE / (3 + ((rng.next() + 1.0) * 1.5) as usize)).max(1);
        let voiced = rng.next() > -0.4;
        let amp = if voiced {
            0.08 + 0.42 * (rng.next() + 1.0) * 0.5
        } else {
            0.0
        };
        let end = (at + syllable).min(out.len());
        for x in &mut out[at..end] {
            // A light low-pass turns white noise into a voice-like broadband carrier.
            smooth = 0.82 * smooth + 0.18 * rng.next();
            *x = amp * smooth;
        }
        at += syllable;
    }
    out
}

fn delayed(input: &[f32], delay_ms: f64) -> Vec<f32> {
    let delay = delay_ms * RATE as f64 / 1000.0;
    let mut out = vec![0.0; input.len()];
    let mut rng = Lcg(0xdecafbad);
    for (i, x) in out.iter_mut().enumerate() {
        let p = i as f64 - delay;
        let clean = if p >= 0.0 {
            let j = p as usize;
            let frac = (p - j as f64) as f32;
            if j + 1 < input.len() {
                input[j] * (1.0 - frac) + input[j + 1] * frac
            } else {
                0.0
            }
        } else {
            0.0
        };
        // Gain, a light low-pass, and -40 dBFS noise.
        *x = 0.3 * clean + 0.01 * rng.next();
    }
    for i in 1..out.len() {
        out[i] = 0.86 * out[i - 1] + 0.14 * out[i];
    }
    out
}

fn envelope(samples: &[f32]) -> (i64, Vec<f32>) {
    let anchors: Vec<Anchor> = (0..samples.len())
        .step_by(480)
        .map(|frame| Anchor {
            frame: frame as u64,
            t_us: frame as i64 * 1_000_000 / RATE as i64,
            frames: (samples.len() - frame).min(480) as u32,
        })
        .collect();
    let mut builder = EnvelopeBuilder::new();
    let (start, mut values) = builder.push(samples, &anchors).expect("bins");
    if let Some((last_start, last)) = builder.finish() {
        assert_eq!(last_start, start + values.len() as i64);
        values.extend(last);
    }
    (start, values)
}

fn estimate(delay_ms: f64) -> LiveEstimate {
    let guest = singing(11);
    let host = delayed(&guest, delay_ms);
    let (gs, ge) = envelope(&guest);
    let (hs, he) = envelope(&host);
    let mut tracker = LiveTracker::new();
    tracker.push_guest(gs, &to_u8(&ge));
    tracker.push_host(hs, &he);
    tracker.estimate(9_500).expect("live estimate")
}

#[test]
fn tracks_synthetic_singing_delays() {
    for delay in [187.3, 40.0, 420.0, 950.0] {
        let found = estimate(delay);
        println!(
            "delay {delay:.1} ms: estimate {:.3} ms, error {:.3} ms",
            found.latency_ms,
            found.latency_ms - delay
        );
        assert!(
            (found.latency_ms - delay).abs() <= 5.0,
            "{delay} ms -> {found:?}"
        );
        assert!(found.ncc >= 0.45, "{found:?}");
    }
}

#[test]
fn rejects_silence_and_periodic_envelopes() {
    let silent = vec![0.0; 11_000];
    let mut tracker = LiveTracker::new();
    tracker.push_guest(0, &to_u8(&silent));
    tracker.push_host(0, &silent);
    assert!(tracker.estimate(9_500).is_none());

    let guest: Vec<f32> = (0..11_000)
        .map(|ms| if (ms % 200) < 100 { 0.3 } else { 0.01 })
        .collect();
    let mut host = vec![0.0; 11_000];
    host[187..].copy_from_slice(&guest[..11_000 - 187]);
    let mut tracker = LiveTracker::new();
    tracker.push_guest(0, &to_u8(&guest));
    tracker.push_host(0, &host);
    assert!(tracker.estimate(9_500).is_none());
}

#[test]
fn filter_requires_two_estimates_and_cools_down() {
    let estimate = |latency_ms| LiveEstimate {
        latency_ms,
        ncc: 0.9,
        voiced: 0.7,
    };
    let mut filter = LiveFilter::new();
    for _ in 0..5 {
        filter.push(estimate(187.0));
        assert!(filter.should_reapply(187.0, 1_000).is_none());
    }
    filter.push(estimate(240.0));
    assert!(filter.should_reapply(187.0, 11_000).is_none());
    filter.push(estimate(240.0));
    assert_eq!(filter.should_reapply(187.0, 11_001), Some(240.0));
    filter.push(estimate(300.0));
    assert!(filter.should_reapply(240.0, 12_000).is_none());
    filter.push(estimate(300.0));
    assert!(filter.should_reapply(240.0, 12_001).is_none());
}

#[test]
fn anchor_gap_keeps_its_position() {
    let mut samples = vec![0.2; 480];
    samples.extend(vec![0.2; 480]);
    let anchors = [
        Anchor {
            frame: 0,
            t_us: 0,
            frames: 480,
        },
        Anchor {
            frame: 96_480,
            t_us: 2_010_000,
            frames: 480,
        },
    ];
    let mut builder = EnvelopeBuilder::new();
    let (start, bins) = builder
        .push(&samples, &anchors)
        .expect("completed first packet");
    assert_eq!(start, 0);
    assert!(bins.len() >= 2_000, "{} bins", bins.len());
    assert!(bins[500..2_000].iter().all(|v| *v == 0.0));
}
