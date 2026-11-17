# Test report

Date: 2026-11-17. Machine: see [PERFORMANCE.md](PERFORMANCE.md).

## Automated tests (`cargo test`): 52 passed, 0 failed (v0.1.1)

v0.1.1 adds the following tests:
- handshake flood cap;
- expired code rejected for new guests while a joined guest resumes;
- reconnect after "New code";
- 10 s late start with a 2 s buffer requests a re-seek instead of over-reporting;
- OBS connection drop mid-apply reports every input;
- ledger recorded before a failed write keeps the original baseline.

| Area | Tests |
| --- | --- |
| Calibration DSP (`tests/calibration.rs`) | known delays 0–777.7 ms within 0.5 ms; gain 0.02–0.9; heavy noise (detected within 5 ms or honestly rejected); pure noise rejected with no value; missing signals (3 of 8 and 8 of 8); false peaks (weaker echo ignored, equal duplicate rejected as ambiguous); clipping flagged; 100 ppm capture-clock error; 44.1→48 kHz resampled capture |
| Clock sync (`network::clocksync`) | symmetric offset; **asymmetric delay (5 / 35 ms) error stays within reported uncertainty**; 80 ppm drift tracked; 30 % packet loss + 80 ms jitter spikes; invalid timestamp rejection |
| Session (`tests/network.rs`, real localhost sockets) | connect + messages + clock sync; invalid join code; expired join code; **host disconnect + guest auto-reconnect** after host restart; guest disconnect detected by host; oversized frame rejected; protocol version mismatch |
| Crypto | join-code round trip; AEAD tamper and replay rejection |
| OBS (`tests/obs.rs`, in-process mock obs-websocket 5 server) | auth success / wrong / missing password; request wire format; offset range check; Render Delay filter create / update / remove; worker apply → **recalibrate twice without stacking** → restore originals with read-back verification; unreachable OBS |
| Compensation | formula; no double compensation; external edit becomes the new baseline; range and duplicate-role errors |
| Playback core | start exactly on schedule; **200 ppm device drift corrected to < 1 ms** (12 ms uncorrected over 60 s); late join skips ahead; clip lands on the exact frame; stale-generation frames discarded; end of track |
| Track | WAV probe, SHA-256 mismatch detection (instrumental file mismatch), streaming from a seek position |
| Audio helpers | anchor regression; **re-gridding packets across silent gaps**; resampler accuracy; Discord root-process selection; unknown audio devices fail cleanly |
| Misc | SPSC ring (bounded, threaded), config round trip, password not persisted, diagnostics sampler and bounded log |

## Real-hardware tests on ONE PC (not a substitute for two-computer tests)

`DuetSync.exe --selftest` (release build):

| Path | Result |
| --- | --- |
| Scheduled presentation → own-process loopback timestamp | +22.76 ms, jitter 0.00 ms, 6/6 sweeps, NCC 1.00 |
| Scheduled presentation → output-device loopback timestamp | +2.28 ms, jitter 0.00 ms, 6/6 sweeps |
| Render → VB-CABLE → capture (3 stream openings) | +20.7 / +14.2 / +19.4 ms, jitter ≤ 0.92 ms, all valid |

`DuetSync.exe --selftest --e2e`: two complete `App` controllers (host + guest) in one
process, connected over 127.0.0.1, with test signals and both players routed to VB-CABLE:

| Step | Result (typical of 5 runs) |
| --- | --- |
| Session + clock sync | offset 0.01–0.03 ms, uncertainty ±0.07–0.23 ms, RTT ~0.2 ms |
| Calibration, guest reference input on | 8/8, −0.02…0.00 ms (host and guest capture the same cable: checks that two independent capture timestamps agree) |
| Calibration, `--no-ref` (scheduled times) | 8/8, **12.05 ms ±0.99 ms**, jitter 0.74 ms (the cable's own latency) |
| Instrumental identity | SHA-256 verified on both sides |
| Synchronized playback | guest-vs-host timeline over ~650 samples: mean −0.06…+0.15 ms, p95 0.55–0.88 ms, worst ≤ 1.02 ms |
| Re-anchored start (50 ms countdown, v0.1.1) | worst 0.76–1.20 ms over ~326 samples |

Bug found and fixed by these hardware runs: loopback and virtual devices deliver **no
packets while silent**. Captured samples were concatenated, so time was lost and later
sweeps appeared up to 450 ms early. The capture is now re-gridded by per-packet QPC
timestamps. A second fix: the position report was timestamped at the start of the buffer
instead of the end, which caused a ~10 ms metric error.

These one-PC results share a single clock and (for playback) a single output device. They
show the pipeline works. They do **not** show two-computer accuracy over the internet.

## Real two-computer checklist (PENDING: not performed)

No second computer was available. Every item below is **pending**.

- [ ] Host and guest on **different networks** (e.g. home Wi-Fi + mobile hotspot) via Tailscale.
- [ ] Discord voice call active during calibration and singing.
- [ ] Each side plays its own local instrumental. Confirm that neither instrumental is audible in Discord.
- [ ] Guest calibrates with speakers→mic, then with a virtual cable; compare the results.
- [ ] Real guest vocal captured by the host's OBS (Application Audio Capture of Discord).
- [ ] OBS recording of a full song with compensation applied.
- [ ] Measure the residual sync error in the recording: clap at a beat on both sides, compare
      the clap onsets with the instrumental in an editor. **Target < 20 ms.**
- [ ] 30+ minute session: recheck the residual error at the end (Discord latency drift);
      record the `guest timeline` values.
- [ ] Listen for echo or duplicated audio (Desktop Audio muted, monitoring off).
- [ ] CPU and RAM on both PCs while OBS + Discord + VTube Studio run.
- [ ] Guest reconnect mid-session (toggle Wi-Fi) and resume.
- [ ] Bluetooth headphones with the output-latency trim.

## How to reproduce

```
cargo test
target\release\DuetSync.exe --selftest
target\release\DuetSync.exe --selftest --cable-test     # needs VB-CABLE
target\release\DuetSync.exe --selftest --e2e [--no-ref] # needs VB-CABLE
```
