# T1: adversarial audit of DuetSync (READ-ONLY)

You are auditing a Rust Windows app (repo root = cwd). Read `docs/ARCHITECTURE.md` and
`CLAUDE.md` first. Do NOT modify files. You may run `cargo test` / `cargo clippy`.

## Scope (read these; skip others unless needed)
- src/audio/wasapi.rs, src/audio/mod.rs (WASAPI capture/render, regrid, anchors)
- src/playback/core.rs, src/playback/track.rs (scheduled start, drift correction, feeder)
- src/calibration/*.rs (chirp, NCC detection, stats)
- src/network/clocksync.rs, src/network/session.rs, src/network/crypto.rs
- src/obs/compensation.rs, src/obs/mod.rs, src/obs/client.rs
- src/app.rs (calibration flow `start_calibration`/`poll_calibration`/`guest_calibrate`,
  playback `host_play`/`guest_play`/`every_second`, OBS apply/restore)

## Look for (real defects only, no style nits)
1. Wrong latency assumptions: is what calibration measures (guest-mic → Discord → host
   process-loopback timestamps) the same quantity OBS needs to compensate? Mixed time bases
   (presentation vs engine/loopback timestamps, QPC vs device clock)?
2. Clock offset estimation / to_local / to_host errors, drift sign errors, uncertainty that
   does not cover real error.
3. Playback drift/scheduling bugs in PlayerCore::fill (sign errors, hard resync, Retime).
4. Audio-thread safety: allocation, blocking locks, I/O or unbounded loops in the render or
   capture threads; ring-buffer memory ordering; Drop/shutdown races; leaked threads.
5. Network: auth/replay/nonce reuse, unbounded queues, reconnect races, secret exposure.
6. OBS: restore/ledger correctness, double compensation, partial-failure handling.
7. Error handling that silently produces wrong numbers instead of reporting failure.

## Output (final message only, max ~900 words)
A list ranked by severity (critical/high/medium). For each finding:
`[severity] file:line — defect — concrete failure scenario — minimal fix`.
Mark anything you are not sure about as "unverified". Then one line: total count by severity.
