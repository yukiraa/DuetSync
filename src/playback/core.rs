//! Real-time player core: schedules track start against presentation time, mixes test
//! clips, measures and gently corrects drift. Pure logic — no OS calls, no allocation in
//! `fill`, unit-testable with a simulated device clock.

use std::sync::Arc;

/// One stereo frame tagged with the seek generation that produced it.
#[derive(Clone, Copy, Default, Debug)]
pub struct Frame {
    pub l: f32,
    pub r: f32,
    pub gen: u32,
}

pub trait FrameSource {
    fn pop(&mut self) -> Option<Frame>;
    fn peek_gen(&self) -> Option<u32>;
    /// True when the producer has reached end of file for `gen`.
    fn ended(&self, gen: u32) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
    Stopped,
    /// Waiting for `start_us` (local clock) to begin at track frame `from`.
    Scheduled,
    Playing,
    Paused,
    Ended,
}

pub enum Command {
    /// Begin track frame `from` audibly at local time `start_us`. Frames must be tagged `gen`.
    Play {
        gen: u32,
        start_us: i64,
        from: u64,
    },
    /// Refresh the anchor (e.g. after clock-offset update) without restarting.
    Retime {
        gen: u32,
        start_us: i64,
    },
    Pause,
    Stop,
    /// Mono clip mixed into both channels, audible at local time `at_us`.
    Clip {
        at_us: i64,
        samples: Arc<[f32]>,
    },
    SetOutputLatency {
        us: i64,
    },
    SetGain {
        gain: f32,
    },
    SetDriftCorrection {
        enabled: bool,
    },
}

struct Clip {
    at_us: i64,
    start_frame: Option<u64>,
    samples: Arc<[f32]>,
}

const MAX_CLIPS: usize = 32;
/// Max playback-rate deviation used for drift correction (±300 ppm ≈ ±0.005 semitone).
pub const MAX_CORRECTION: f64 = 300e-6;
/// Drift beyond this is fixed by an immediate jump instead of rate correction.
pub const HARD_RESYNC_MS: f64 = 40.0;

pub struct PlayerCore {
    pub rate: f64,
    pub state: State,
    gen: u32,
    start_us: i64,
    from: u64,
    /// Track position (frames) of the next output frame.
    pos: f64,
    cur: Frame,
    next: Frame,
    frac: f64,
    pop_budget: usize,
    drift_ema: f64,
    pub drift_correction: bool,
    pub output_latency_us: i64,
    pub gain: f32,
    clips: [Option<Clip>; MAX_CLIPS],
    pub underruns: u64,
    pub hard_resyncs: u64,
    /// The feeder must seek to this track frame before playback can resume.
    pub needs_reseek: Option<u64>,
    pub peak: f32,
}

impl PlayerCore {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as f64,
            state: State::Stopped,
            gen: 0,
            start_us: 0,
            from: 0,
            pos: 0.0,
            cur: Frame::default(),
            next: Frame::default(),
            frac: 0.0,
            pop_budget: 0,
            drift_ema: 0.0,
            drift_correction: true,
            output_latency_us: 0,
            gain: 1.0,
            clips: Default::default(),
            underruns: 0,
            hard_resyncs: 0,
            needs_reseek: None,
            peak: 0.0,
        }
    }

    pub fn position_frames(&self) -> f64 {
        self.pos
    }
    pub fn drift_ms(&self) -> f64 {
        self.drift_ema * 1000.0 / self.rate
    }
    pub fn gen(&self) -> u32 {
        self.gen
    }

    /// Apply a command. Returns a clip that could not be queued (caller drops it off-thread).
    pub fn command(&mut self, c: Command) -> Option<Arc<[f32]>> {
        match c {
            Command::Play {
                gen,
                start_us,
                from,
            } => {
                self.gen = gen;
                self.start_us = start_us;
                self.from = from;
                self.pos = from as f64;
                self.frac = 0.0;
                self.drift_ema = 0.0;
                self.needs_reseek = None;
                self.state = State::Scheduled;
            }
            Command::Retime { gen, start_us } => {
                if gen == self.gen {
                    self.start_us = start_us;
                }
            }
            Command::Pause => {
                if matches!(self.state, State::Playing | State::Scheduled) {
                    self.state = State::Paused;
                }
            }
            Command::Stop => {
                self.state = State::Stopped;
                self.pos = 0.0;
            }
            Command::Clip { at_us, samples } => {
                if let Some(slot) = self.clips.iter_mut().find(|s| s.is_none()) {
                    *slot = Some(Clip {
                        at_us,
                        start_frame: None,
                        samples,
                    });
                } else {
                    return Some(samples);
                }
            }
            Command::SetOutputLatency { us } => self.output_latency_us = us,
            Command::SetGain { gain } => self.gain = gain.clamp(0.0, 2.0),
            Command::SetDriftCorrection { enabled } => self.drift_correction = enabled,
        }
        None
    }

    /// Audible local time of stream frame `n`, given that `n0` is presented at `t0_us`.
    #[inline]
    fn audible(&self, n: u64, n0: u64, t0_us: f64) -> f64 {
        t0_us + (n as f64 - n0 as f64) * 1e6 / self.rate + self.output_latency_us as f64
    }

    fn pop_gen<S: FrameSource>(&mut self, src: &mut S) -> Option<Frame> {
        while self.pop_budget > 0 {
            self.pop_budget -= 1;
            match src.pop() {
                Some(f) if f.gen == self.gen => return Some(f),
                Some(_) => continue,
                None => return None,
            }
        }
        None
    }

    /// Drop at most one second of queued audio. The caller re-seeks if this cannot
    /// reach the requested frame, rather than claiming a position the source did not
    /// provide.
    fn skip_frames<S: FrameSource>(&mut self, src: &mut S, requested: u64) -> u64 {
        let mut consumed = 0;
        for _ in 0..requested.min(self.rate as u64) {
            if self.pop_gen(src).is_none() {
                break;
            }
            consumed += 1;
        }
        consumed
    }

    /// Fill interleaved stereo `out` whose first frame is stream frame `n0`, presented at
    /// local time `t0_us`. Returns finished clips to drop outside the audio thread.
    pub fn fill<S: FrameSource>(
        &mut self,
        out: &mut [f32],
        n0: u64,
        t0_us: f64,
        src: &mut S,
        finished: &mut impl FnMut(Arc<[f32]>),
    ) {
        let frames = out.len() / 2;
        out.fill(0.0);
        self.pop_budget = self.rate as usize;
        if self.needs_reseek.is_some() {
            self.peak = 0.0;
            return;
        }
        let a0 = self.audible(n0, n0, t0_us);
        let mut i = 0usize;

        if self.state == State::Scheduled {
            let wait = ((self.start_us as f64 - a0) * self.rate / 1e6).round();
            if wait >= frames as f64 {
                i = frames; // whole buffer silent
            } else {
                if wait > 0.0 {
                    i = wait as usize;
                } else {
                    // Late (joined mid-song or slow start): skip ahead to stay aligned.
                    let skip = (-wait) as u64;
                    let consumed = self.skip_frames(src, skip);
                    self.pos = self.from.saturating_add(consumed) as f64;
                    if consumed != skip {
                        self.needs_reseek = Some(self.from.saturating_add(skip));
                        self.peak = 0.0;
                        return;
                    }
                }
                self.prime(src);
                self.state = State::Playing;
            }
        }

        if self.state == State::Playing {
            // Drift: actual track position vs. ideal for the audible time of frame n0+i.
            let ideal = self.from as f64
                + (self.audible(n0 + i as u64, n0, t0_us) - self.start_us as f64) * self.rate / 1e6;
            let drift = self.pos - ideal;
            self.drift_ema += 0.1 * (drift - self.drift_ema);
            let hard = HARD_RESYNC_MS * self.rate / 1000.0;
            if drift.abs() > hard {
                self.hard_resyncs += 1;
                if drift > 0.0 {
                    // Ahead: hold silence for `drift` frames.
                    let hold = (drift as usize).min(frames - i);
                    i += hold;
                    self.drift_ema = drift - hold as f64;
                } else {
                    let skip = (-drift) as u64;
                    let target = (self.pos.max(0.0) as u64).saturating_add(skip);
                    let consumed = self.skip_frames(src, skip);
                    self.pos += consumed as f64;
                    if consumed != skip {
                        self.needs_reseek = Some(target);
                        self.peak = 0.0;
                        return;
                    }
                    self.prime(src);
                    self.drift_ema = 0.0;
                }
            }
            let step = if self.drift_correction && self.drift_ema.abs() > 0.25e-3 * self.rate {
                1.0 - (self.drift_ema / (self.rate * 2.0)).clamp(-MAX_CORRECTION, MAX_CORRECTION)
            } else {
                1.0
            };
            let g = self.gain;
            while i < frames {
                let l = self.cur.l + (self.next.l - self.cur.l) * self.frac as f32;
                let r = self.cur.r + (self.next.r - self.cur.r) * self.frac as f32;
                out[2 * i] = l * g;
                out[2 * i + 1] = r * g;
                i += 1;
                self.frac += step;
                self.pos += step;
                while self.frac >= 1.0 {
                    self.frac -= 1.0;
                    self.cur = self.next;
                    match self.pop_gen(src) {
                        Some(f) => self.next = f,
                        None => {
                            if src.ended(self.gen) {
                                self.state = State::Ended;
                            } else {
                                self.underruns += 1;
                            }
                            self.next = Frame::default();
                            break;
                        }
                    }
                }
                if self.state != State::Playing {
                    break;
                }
            }
        }
        if self.state != State::Playing {
            // Purge frames of older generations so the feeder can refill for the next start.
            while self.pop_budget > 0 {
                match src.peek_gen() {
                    Some(g) if g != self.gen => {
                        self.pop_budget -= 1;
                        src.pop();
                    }
                    _ => break,
                }
            }
        }

        // Mix scheduled clips.
        let end = n0 + frames as u64;
        for slot in self.clips.iter_mut() {
            let Some(c) = slot else { continue };
            if c.start_frame.is_none() {
                let lead =
                    (c.at_us as f64 - (t0_us + self.output_latency_us as f64)) * self.rate / 1e6;
                if lead < self.rate {
                    c.start_frame = Some((n0 as f64 + lead.max(0.0)).round() as u64);
                }
            }
            if let Some(s) = c.start_frame {
                let clip_end = s + c.samples.len() as u64;
                let from = s.max(n0);
                let to = clip_end.min(end);
                for n in from..to {
                    let v = c.samples[(n - s) as usize];
                    let k = (n - n0) as usize;
                    out[2 * k] += v;
                    out[2 * k + 1] += v;
                }
                if clip_end <= end {
                    if let Some(c) = slot.take() {
                        finished(c.samples);
                    }
                }
            }
        }
        let mut pk = 0f32;
        for v in out.iter() {
            pk = pk.max(v.abs());
        }
        self.peak = pk;
    }

    fn prime<S: FrameSource>(&mut self, src: &mut S) {
        self.cur = self.pop_gen(src).unwrap_or_default();
        self.next = self.pop_gen(src).unwrap_or_default();
        self.frac = 0.0;
    }

    /// Local time at which clip/track frame scheduling considers `n0` audible.
    pub fn audible_now(&self, n0: u64, t0_us: f64) -> f64 {
        self.audible(n0, n0, t0_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Track source producing a ramp so positions are checkable: sample value = frame index.
    struct Ramp {
        next: u64,
        gen: u32,
        len: u64,
    }
    impl FrameSource for Ramp {
        fn pop(&mut self) -> Option<Frame> {
            if self.next >= self.len {
                return None;
            }
            let v = self.next as f32;
            self.next += 1;
            Some(Frame {
                l: v,
                r: v,
                gen: self.gen,
            })
        }
        fn ended(&self, _g: u32) -> bool {
            self.next >= self.len
        }
        fn peek_gen(&self) -> Option<u32> {
            (self.next < self.len).then_some(self.gen)
        }
    }

    /// Simulate a device whose sample clock runs at `ppm` relative to local time.
    fn simulate(ppm: f64, correction: bool, seconds: f64) -> (PlayerCore, Vec<f64>) {
        let rate = 48_000u32;
        let mut core = PlayerCore::new(rate);
        core.drift_correction = correction;
        let start_us = 500_000i64;
        let from = 1000u64;
        let mut src = Ramp {
            next: from,
            gen: 1,
            len: u64::MAX,
        };
        core.command(Command::Play {
            gen: 1,
            start_us,
            from,
        });
        let buf = 480usize;
        let mut out = vec![0f32; buf * 2];
        let mut n0 = 0u64;
        let mut errs = vec![];
        let true_rate = rate as f64 * (1.0 + ppm * 1e-6);
        while (n0 as f64) < seconds * true_rate {
            // Presentation time of n0 according to the *device* clock (true rate).
            let t0 = n0 as f64 * 1e6 / true_rate;
            core.fill(&mut out, n0, t0, &mut src, &mut |_| {});
            if core.state == State::Playing {
                // Error = played sample value vs. ideal track frame at that true time.
                let ideal = from as f64 + (t0 - start_us as f64) * rate as f64 / 1e6;
                let played = out[0] as f64;
                if played > 0.0 {
                    errs.push((played - ideal) * 1000.0 / rate as f64);
                }
            }
            n0 += buf as u64;
        }
        (core, errs)
    }

    #[test]
    fn starts_on_schedule() {
        let (_, errs) = simulate(0.0, true, 2.0);
        assert!(!errs.is_empty());
        assert!(errs.iter().all(|e| e.abs() < 0.05), "{:?}", &errs[..5]);
    }

    #[test]
    fn device_drift_is_corrected_gradually() {
        // 200 ppm over 60 s would be 12 ms uncorrected.
        let (core, errs) = simulate(200.0, false, 60.0);
        let last = *errs.last().unwrap();
        assert!(last.abs() > 10.0, "uncorrected drift {last}");
        assert!(core.drift_ms().abs() > 10.0 || core.hard_resyncs == 0);

        let (core, errs) = simulate(200.0, true, 60.0);
        let worst_late = errs[errs.len() / 2..]
            .iter()
            .fold(0f64, |a, e| a.max(e.abs()));
        assert!(worst_late < 1.0, "corrected residual {worst_late} ms");
        assert_eq!(core.hard_resyncs, 0);
    }

    #[test]
    fn late_start_skips_ahead() {
        let rate = 48_000u32;
        let mut core = PlayerCore::new(rate);
        let mut src = Ramp {
            next: 0,
            gen: 3,
            len: u64::MAX,
        };
        core.command(Command::Play {
            gen: 3,
            start_us: 0,
            from: 0,
        });
        let mut out = vec![0f32; 960];
        // First buffer presented 100 ms after the scheduled start.
        core.fill(&mut out, 0, 100_000.0, &mut src, &mut |_| {});
        assert_eq!(core.state, State::Playing);
        assert_eq!(core.needs_reseek, None);
        assert!((out[0] - 4800.0).abs() <= 1.0, "{}", out[0]);
    }

    #[test]
    fn large_late_start_requests_reseek_without_overreporting_position() {
        let rate = 48_000u32;
        let mut core = PlayerCore::new(rate);
        let mut src = Ramp {
            next: 0,
            gen: 4,
            len: 2 * rate as u64,
        };
        core.command(Command::Play {
            gen: 4,
            start_us: 0,
            from: 0,
        });
        let mut out = vec![1.0f32; 960];

        core.fill(&mut out, 0, 10_000_000.0, &mut src, &mut |_| {});

        assert_eq!(core.needs_reseek, Some(10 * rate as u64));
        assert_eq!(core.position_frames(), rate as f64);
        assert!(core.position_frames() <= src.next as f64);
        assert!(out.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn clip_scheduled_at_exact_frame() {
        let mut core = PlayerCore::new(48_000);
        let clip: Arc<[f32]> = vec![1.0f32; 10].into();
        core.command(Command::Clip {
            at_us: 15_000,
            samples: clip,
        });
        let mut src = Ramp {
            next: 0,
            gen: 0,
            len: 0,
        };
        let mut out = vec![0f32; 960 * 2];
        let mut done = 0;
        core.fill(&mut out, 0, 0.0, &mut src, &mut |_| done += 1);
        let first = out.iter().position(|v| *v > 0.5).unwrap() / 2;
        assert_eq!(first, 720); // 15 ms * 48 kHz
        assert_eq!(done, 1);
    }

    #[test]
    fn stale_generation_discarded_and_end_detected() {
        let mut core = PlayerCore::new(48_000);
        let mut src = Ramp {
            next: 0,
            gen: 1,
            len: 100,
        };
        core.command(Command::Play {
            gen: 2,
            start_us: 0,
            from: 0,
        });
        let mut out = vec![0f32; 960];
        core.fill(&mut out, 0, 0.0, &mut src, &mut |_| {});
        assert!(out.iter().all(|v| *v == 0.0));
        assert_eq!(core.state, State::Ended);
    }
}
