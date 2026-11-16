//! NTP-style clock offset estimation between guest (local) and host (reference).
//!
//! For one exchange: t1 guest send, t2 host receive, t3 host send, t4 guest receive.
//!   offset θ = ((t2 - t1) + (t3 - t4)) / 2      (host = guest + θ)
//!   delay  δ = (t4 - t1) - (t3 - t2)
//! If the path is asymmetric the true offset lies in θ ± δ/2, so δ/2 is reported as the
//! (worst-case) uncertainty rather than pretending half the RTT is the one-way delay.

#[derive(Clone, Copy, Debug)]
pub struct SyncSample {
    pub t_local: i64,
    pub offset: f64,
    pub rtt: f64,
}

pub fn sample(t1: i64, t2: i64, t3: i64, t4: i64) -> Option<SyncSample> {
    let rtt = ((t4 - t1) - (t3 - t2)) as f64;
    if rtt < 0.0 || t3 < t2 || t4 < t1 {
        return None;
    }
    Some(SyncSample {
        t_local: t1 + (t4 - t1) / 2,
        offset: ((t2 - t1) + (t3 - t4)) as f64 / 2.0,
        rtt,
    })
}

#[derive(Clone, Debug)]
pub struct Estimate {
    /// host_us = local_us + offset_us (at `at_local`), slope in µs per µs.
    pub offset_us: f64,
    pub drift_ppm: f64,
    pub at_local: i64,
    pub uncertainty_us: f64,
    pub rtt_min_us: f64,
    pub rtt_median_us: f64,
    pub samples: usize,
}

impl Estimate {
    pub fn to_host(&self, local_us: i64) -> i64 {
        (local_us as f64
            + self.offset_us
            + self.drift_ppm * 1e-6 * (local_us - self.at_local) as f64)
            .round() as i64
    }
    pub fn to_local(&self, host_us: i64) -> i64 {
        // Invert to first order (drift ≤ a few hundred ppm).
        let approx = host_us as f64 - self.offset_us;
        (approx - self.drift_ppm * 1e-6 * (approx - self.at_local as f64)).round() as i64
    }
}

pub struct ClockSync {
    window: Vec<SyncSample>,
    cap: usize,
    pub received: u64,
    pub rejected: u64,
}

impl Default for ClockSync {
    fn default() -> Self {
        Self::new(240)
    }
}

impl ClockSync {
    pub fn new(cap: usize) -> Self {
        Self {
            window: Vec::with_capacity(cap),
            cap,
            received: 0,
            rejected: 0,
        }
    }

    pub fn clear(&mut self) {
        self.window.clear();
    }

    pub fn add(&mut self, t1: i64, t2: i64, t3: i64, t4: i64) {
        match sample(t1, t2, t3, t4) {
            Some(s) => {
                self.received += 1;
                if self.window.len() == self.cap {
                    self.window.remove(0);
                }
                self.window.push(s);
            }
            None => self.rejected += 1,
        }
    }

    pub fn estimate(&self) -> Option<Estimate> {
        if self.window.len() < 4 {
            return None;
        }
        let mut rtts: Vec<f64> = self.window.iter().map(|s| s.rtt).collect();
        rtts.sort_by(f64::total_cmp);
        let rtt_min = rtts[0];
        let rtt_med = rtts[rtts.len() / 2];
        // Keep the fastest third (at least 4): queueing delay only ever adds latency.
        let thresh = rtts[(rtts.len() / 3).max(3).min(rtts.len() - 1)];
        let good: Vec<&SyncSample> = self.window.iter().filter(|s| s.rtt <= thresh).collect();
        let t0 = good.last().map(|s| s.t_local).unwrap_or(0);
        let n = good.len() as f64;
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for s in &good {
            let x = (s.t_local - t0) as f64;
            sx += x;
            sy += s.offset;
            sxx += x * x;
            sxy += x * s.offset;
        }
        let den = n * sxx - sx * sx;
        let span = (good.last().unwrap().t_local - good[0].t_local) as f64;
        // Only trust a slope over ≥ 20 s of data; otherwise assume zero drift.
        let slope = if span >= 20e6 && den.abs() > 1e-6 {
            ((n * sxy - sx * sy) / den).clamp(-500e-6, 500e-6)
        } else {
            0.0
        };
        let intercept = (sy - slope * sx) / n;
        let resid = (good
            .iter()
            .map(|s| {
                let p = intercept + slope * (s.t_local - t0) as f64;
                (s.offset - p).powi(2)
            })
            .sum::<f64>()
            / n)
            .sqrt();
        let best_rtt = good.iter().map(|s| s.rtt).fold(f64::INFINITY, f64::min);
        Some(Estimate {
            offset_us: intercept,
            drift_ppm: slope * 1e6,
            at_local: t0,
            uncertainty_us: best_rtt / 2.0 + 2.0 * resid,
            rtt_min_us: rtt_min,
            rtt_median_us: rtt_med,
            samples: self.window.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn unit(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Simulate exchanges every 500 ms. Host clock = guest·(1+ppm) + offset.
    fn sim(
        offset: f64,
        ppm: f64,
        up_ms: f64,
        down_ms: f64,
        jitter_ms: f64,
        loss: f64,
        secs: f64,
    ) -> (ClockSync, f64, f64) {
        let mut cs = ClockSync::default();
        let mut rng = Lcg(7);
        let host = |g: f64| g * (1.0 + ppm * 1e-6) + offset;
        let mut t = 1e6;
        while t < secs * 1e6 {
            let up = up_ms * 1e3 + rng.unit() * jitter_ms * 1e3;
            let down = down_ms * 1e3 + rng.unit() * jitter_ms * 1e3;
            if rng.unit() >= loss {
                let t1 = t;
                let t2 = host(t1 + up);
                let t3 = t2 + 200.0;
                // guest receives after `down` of guest time (approximately)
                let t4 = t1 + up + 200.0 / (1.0 + ppm * 1e-6) + down;
                cs.add(t1 as i64, t2 as i64, t3 as i64, t4 as i64);
            }
            t += 500e3;
        }
        (cs, t, host(t))
    }

    #[test]
    fn symmetric_offset_recovered() {
        let (cs, now, host_now) = sim(123_456_789.0, 0.0, 10.0, 10.0, 2.0, 0.0, 30.0);
        let e = cs.estimate().unwrap();
        let err = e.to_host(now as i64) as f64 - host_now;
        assert!(err.abs() < 1500.0, "err {err} µs");
        assert!(e.uncertainty_us >= err.abs());
    }

    #[test]
    fn asymmetric_delay_is_within_reported_uncertainty() {
        // 5 ms up, 35 ms down: true error of the naive estimate is 15 ms.
        let (cs, now, host_now) = sim(-5e6, 0.0, 5.0, 35.0, 1.0, 0.0, 30.0);
        let e = cs.estimate().unwrap();
        let err = (e.to_host(now as i64) as f64 - host_now).abs();
        assert!(err > 10_000.0, "asymmetry should bias the estimate ({err})");
        assert!(
            e.uncertainty_us >= err,
            "uncertainty {} must cover error {err}",
            e.uncertainty_us
        );
    }

    #[test]
    fn drift_tracked() {
        // 80 ppm relative drift over 120 s would be 9.6 ms if ignored.
        let (cs, now, host_now) = sim(1e6, 80.0, 8.0, 8.0, 3.0, 0.0, 120.0);
        let e = cs.estimate().unwrap();
        assert!((e.drift_ppm - 80.0).abs() < 10.0, "ppm {}", e.drift_ppm);
        let err = (e.to_host(now as i64) as f64 - host_now).abs();
        assert!(err < 2000.0, "err {err}");
        let back = e.to_local(e.to_host(now as i64));
        assert!((back - now as i64).abs() <= 2);
    }

    #[test]
    fn lost_and_delayed_messages() {
        // 30 % loss, heavy jitter spikes (up to 80 ms): min-RTT filtering keeps accuracy.
        let (cs, now, host_now) = sim(42e6, 0.0, 10.0, 10.0, 80.0, 0.3, 60.0);
        let e = cs.estimate().unwrap();
        let err = (e.to_host(now as i64) as f64 - host_now).abs();
        assert!(
            err <= e.uncertainty_us,
            "err {err} unc {}",
            e.uncertainty_us
        );
        assert!(e.samples < 120);
        assert!(sample(10, 5, 4, 20).is_none(), "t3 < t2 rejected");
    }
}
