# DuetSync architecture

Single Cargo crate: `src/lib.rs` (all logic, testable) + `src/main.rs` (GUI / `--selftest` entry).

## Timing model
* One monotonic timebase per machine: `clock::now_us()` = QueryPerformanceCounter in µs.
  WASAPI QPC timestamps (100 ns units) map onto the same base (`qpc100ns / 10`).
* Host clock is the session reference. Guest estimates `offset = host - guest` with
  NTP-style 4-timestamp exchanges (`network::clocksync`). Only low-RTT samples are used;
  uncertainty is reported as `rtt_min / 2` (worst case for fully asymmetric paths), never
  as an exact one-way delay. Drift is a least-squares slope over the sample window.

## What is measured
Audience output is aligned in OBS on the **host** machine:
* host mic and host instrumental are local (relative latency ≈ 0, user may fine-tune);
* guest voice arrives through Discord with latency `L` = guest-mic → host Discord render.

Assisted calibration: host schedules N chirps at host-clock times `T_k`. The guest app
plays them (presentation-time scheduled) on a user-selected output; the guest optionally
records its own mic to timestamp when each chirp actually entered the mic (`G_k`,
converted to host clock). The host captures Discord via process loopback and finds
arrivals `A_k` via normalized cross-correlation. `L_k = A_k - (G_k or T_k)`.
Result = median, jitter (MAD-based), confidence, plus clock uncertainty. Weak, ambiguous,
missing or clipped detections are rejected, never invented.

## OBS compensation
`target = max(latency_i) + margin`, `offset_i = baseline_i + (target - latency_i)`.
`baseline_i` is the user's pre-DuetSync offset, captured once and persisted, so repeated
calibration never stacks. Restore writes baselines back. Values are read back to verify.

## Playback
Both machines play their own copy of the instrumental (SHA-256 verified). The host sends
`Play { start_host_us, from_frame }` a few seconds in the future. Each render engine maps
WASAPI `IAudioClock` position + QPC to the presentation time of every frame it writes and
starts the track exactly at the scheduled time (minus a user output-latency trim).
Drift vs. the QPC timeline is measured and reported; resync re-anchors.

## Threads
* UI thread (egui, reactive repaint ≤ 4 Hz while idle).
* WASAPI capture/render threads (event-driven, no allocation, no blocking locks).
* Network: reader + writer thread per connection (blocking sockets with timeouts).
* OBS worker thread owning the WebSocket.
* Calibration analysis on a short-lived worker thread.

## Transport security
Join code = 128-bit random secret (+ expiry). Mutual HMAC-SHA256 challenge/response,
then ChaCha20-Poly1305 per-direction keys with counter nonces. Frames ≤ 64 KiB.
