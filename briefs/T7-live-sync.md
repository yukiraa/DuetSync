# T7: live Discord-delay tracking from real singing (implementation)

Repo root = cwd. Read `CLAUDE.md` and `docs/ARCHITECTURE.md` first. Rust, Windows.
Use `$env:CARGO_TARGET_DIR="target-t7"` for every cargo command (another agent builds in
parallel). No new crates (no network in the sandbox).

## Why
The calibration measures the guest's voice delay once (guest mic to Discord to host capture).
Discord's delay drifts during a song. The user chose: **keep measuring from the real singing**
and re-apply the OBS delay automatically when it changes by more than ~30 ms. Network ping is
NOT used (it is not Discord's audio delay).

Method: the guest computes a compact voice-energy envelope of their own mic (timestamped on the
host clock via the existing clock sync) and sends it to the host. The host computes the same
envelope of the Discord capture and cross-correlates the two envelopes. The lag is the current
delay, in exactly the time base the calibration uses (mic capture timestamps vs host capture
timestamps).

## Files you own (modify ONLY these)
- `src/live/mod.rs` (stub exists) plus new files under `src/live/`
- `src/audio/wasapi.rs`: ADD a streaming API to `CaptureHandle` only (do not change existing behaviour)
- `tests/live.rs` (new)

## Build
1. **wasapi.rs:** add `CaptureHandle::set_streaming(&self, on: bool)` (turns the existing
   `recording` flag on/off) and `CaptureHandle::drain(&mut self, samples: &mut Vec<f32>,
   anchors: &mut Vec<Anchor>) -> Result<(), String>`. It is non-blocking, empties both rings,
   and returns `Err` on capture error or overflow (dropped counters increased). `record_until`
   must keep working unchanged.
2. **`live/envelope.rs`:** `EnvelopeBuilder`.
   - Input: mono 48 kHz samples + `Anchor`s (frame index, capture time in µs, frames).
   - Band-pass 300–3400 Hz (two biquads), then **1 ms bins** of RMS, placed on the timeline by
     the anchors' timestamps. Fill gaps (no packets while silent) with zeros, like
     `audio::regrid`.
   - Output: completed bins as `(start_ms: i64, Vec<f32>)` in the capture's local clock (ms).
   - Quantize for transport: `to_u8(&[f32]) -> Vec<u8>` (log scale, -70..0 dBFS to 0..255) and
     `from_u8`.
   - No allocation per sample; reuse buffers.
3. **`live/tracker.rs`:**
   - `LiveTracker` keeps two bounded time series (last 12 s) on the HOST clock in ms: `guest`
     (pushed as u8 slices with `start_host_ms`) and `host` (f32 slices from the host's
     envelope, `start_ms` = host local ms).
   - `estimate(now_ms) -> Option<LiveEstimate { latency_ms: f64, ncc: f32, voiced: f32 }>`:
     - Guest window = 4000 ms ending at `now_ms - 1300`.
     - Lags 0..1000 ms in 1 ms steps; z-normalise both; normalized cross-correlation; parabolic
       sub-bin interpolation.
     - Reject (None) when: the guest window is mostly silent (voiced fraction < 0.2 or low
       variance); NCC < 0.45; peak ratio < 1.15 against the best peak outside ±40 ms; or data
       is missing.
   - `LiveFilter`: median of the last 5 accepted estimates;
     `should_reapply(applied_ms: f64, now_ms) -> Option<f64>` returns the new median when
     |median - applied| > 30 ms for 2 consecutive accepted estimates AND at least 10 s have
     passed since the last re-apply (then remember it).
4. **`live/worker.rs`:** `LiveCapture::start(source: CaptureSource) -> Result<LiveCapture, String>`.
   - Spawns one thread that owns a streaming `CaptureHandle`, drains every 100 ms (sleep, not
     busy-wait) and feeds an `EnvelopeBuilder`.
   - Completed bins go out on a bounded `sync_channel(64)` (drop on full, count drops).
   - `try_recv()` plus `error()`; stops on Drop (join the thread).
5. **`live/mod.rs`:** `pub mod` + re-exports, and a short module doc that explains the method
   and its limits (needs singing on both sides of the path; Krisp may distort the envelope).

## Tests (`tests/live.rs`, deterministic; no audio devices)
Synthesize "singing": band-limited noise amplitude-modulated by random syllables (3–6 Hz, 70 %
voiced, seeded LCG) at 48 kHz with anchors every 480 frames.
- Host signal = guest delayed by 187.3 ms, gain 0.3, light low-pass, plus noise at -40 dB.
  Run both through `EnvelopeBuilder`, push to `LiveTracker` (guest via `to_u8`), and
  `estimate` gives 187.3 ±5 ms.
- Delays of 40, 420 and 950 ms are also recovered within ±5 ms.
- Guest silent: `None`.
- A strictly periodic guest envelope (ambiguous): `None`.
- `LiveFilter`: a stable 187 ms series with applied 187 gives no re-apply. A step to 240 ms
  re-applies after the 2nd consecutive estimate. A second step within 10 s is held.
- A silent gap (missing anchors for 2 s) does not shift the timeline.

## Gate
`cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), `cargo test` all pass (with
`CARGO_TARGET_DIR=target-t7`). The capture/drain code runs off the real-time thread; the
capture thread itself is unchanged.

## Final message (max 250 words)
Files and public API, the test list with results, and the measured estimate errors from the
tests (actual numbers).
