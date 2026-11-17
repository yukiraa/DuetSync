# T10: fix YouTube drift correction (seek latency) — real-browser data inside

Repo root = cwd. Read `CLAUDE.md`, `src/video/server.rs`, `tests/video.rs`. Use
`$env:CARGO_TARGET_DIR="target-t10"`. No new crates.

## Real measurement (Chrome-based browser, real YouTube embed, localhost RTT 0.23 ms)
`DuetSync.exe --selftest --video-test <link>` scheduled play at +3 s from 30.0 s and sampled
`ClientInfo.error_ms` every 500 ms (negative = video behind the shared timeline):

```
start:            -152.7 -151.8 -151.4   (playVideo start-up latency)
seek -> buffering, then -169.8 -171.0 -170.2
seek -> buffering, then  -87.3  -88.0  -88.1
seek -> buffering, then -105.6 -106.4 -106.5
seek -> buffering, then -103.8 -103.9
seek -> buffering, then -119.4 -120.3 -121.1
seek -> buffering, then -118.4 -118.9
median |error| 118.5 ms, p95 170.4 ms -> FAIL (target: median <= 40 ms)
```

Diagnosis:
- `seek` goes to `desired` "now", but a YouTube seek takes ~90–170 ms to resume, so it always
  lands behind by about the seek latency.
- The 3 s cooldown then fires another seek, each with an audible/visible stall.
- The start has the same problem (play start-up latency).

## Files you own
`src/video/server.rs`, `tests/video.rs`. Keep the public API used by `src/app.rs` unchanged
(adding fields or methods is fine).

## Required behaviour
1. **Learned seek lead per client.** Correction seeks go to `desired + lead_s`.
   - After a correction, wait until the client reports `playing` again and at least 1 s of
     stable reports have passed. Then measure the residual error `e`.
   - Update `lead_s += -e * 0.8`, clamped to 0..0.6 s.
   - Start with `lead_s = 0.12`. Reset it only on a new `load`.
2. **Start-up lead.** `play_at` sends `play_at` with `page_ms` earlier by a learned
   `start_lead_ms` (initial 120 ms, updated from the residual error measured 1–2 s after the
   start, same gain and clamp 0..400 ms). Seek to `from_s` at that earlier time as well, so the
   timeline is unchanged.
3. **Fewer, gentler corrections.**
   - Correct only when |err| > 60 ms for 2 consecutive reports (no single-sample triggers),
     never while `buffering`/`ad`/`paused`.
   - Cooldown: 4 s after a correction, measured from when playing resumes.
   - After 3 corrections within 30 s, raise the threshold to 120 ms for 30 s to avoid seek storms.
4. Keep `error_ms` reporting (UI uses it). Add `seek_lead_ms` and `corrections` to `ClientInfo`
   for diagnostics.

## Tests (pure policy; deterministic)
Model the player in a test:
- a seek resumes after a latency of 120 ms ± 20 ms jitter (seeded), position frozen meanwhile;
- the start has a latency of 150 ms;
- reports every 250 ms with ±3 ms noise.

Required results:
- From a start error of −150 ms, steady-state |error| ≤ 40 ms within 3 corrections and stays
  there for 60 simulated seconds with no further seeks.
- A constant clock drift of 300 ppm needs at most 1 correction per 60 s once learned.
- No seek while buffering, and 2 consecutive out-of-threshold reports are required.
- The storm guard triggers after 3 corrections in 30 s.

Keep the existing tests green.

## Gate
`cargo fmt --check`, `cargo clippy --all-targets` 0 warnings, `cargo test` all pass (with
`CARGO_TARGET_DIR=target-t10`).

## Final message (max 200 words)
What changed, the simulated convergence numbers, and the test results.
