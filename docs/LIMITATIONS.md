# Known limitations

**Not production-proven.** Real two-computer testing is still pending
(see [TEST_REPORT.md](TEST_REPORT.md)).

## Latency and Discord

- DuetSync does not remove latency. Performers still hear each other late in Discord.
- Compensation from a calibration is **static**. Discord's jitter buffer changes its delay
  during a song. Auto-sync can follow it, but only from real singing (see below). The safety margin absorbs small changes but does not track
  them. Recalibrate between songs.
- Calibration needs Discord's Noise Suppression (Krisp) and Echo Cancellation turned off
  temporarily. Otherwise the test sweeps may be removed.
- The test signal must actually enter the guest's Discord microphone: through speakers near
  the mic, or through a virtual cable. Headphones alone cannot do this.
- Without the guest's reference input, the measurement includes the guest's
  speaker→microphone acoustic delay (≈ 1 ms per 34 cm) and any unreported output latency.
- Network ping is shown for information only. It is never used as the audio latency.
- The measured latency is in the time base of the host's capture tap. "Discord app"
  (process loopback) taps the same stream that OBS Application Audio Capture uses.
  "Output device" loopback taps later, after the device buffer, so it can over-delay the
  other OBS sources by a few to tens of ms. Prefer "Discord app", and verify with a clap
  test.

## Clock and playback

- Clock offset uncertainty is reported as half the best round trip (worst case for
  asymmetric routes). Over the internet expect ±5–30 ms, which adds to the playback error
  between the two performers.
- Output latency not reported by Windows (Bluetooth, some USB DACs) must be entered
  manually as the *Output latency trim*.
- Drift correction uses tiny playback-rate changes (≤ ±300 ppm; inaudible pitch change).
  Larger errors (> 40 ms) jump.
- Instrumentals: **WAV only** (PCM 8–32-bit, float32). Convert other formats first.

## Networking

- Internet joining uses hole punching with free public helpers (Google STUN, ntfy.sh relay for the encrypted address exchange). Very strict networks (some mobile data / carrier-grade NAT on both sides) cannot be punched; there is no fallback relay for the session itself. If ntfy.sh or STUN is down, only same-Wi-Fi joining works.
- The transport is authenticated and encrypted with keys derived from the 128-bit join
  code. The code admits new guests only until it expires. After joining, a guest reconnects
  with a per-session resume credential, so expiry or "New code" doesn't drop them.
- Exactly two performers.

## OBS and video

- Requires OBS WebSocket 5 (OBS 28+). OBS is reachable only on loopback or private
  addresses.
- Audio sync offset range: −950…20000 ms. Render Delay filter: 0…500 ms.
- Video compensation is **manual**. Video latency is not measured.

## Platform and resources

- Windows only. Process loopback needs Windows 10 2004+.
- No system tray icon. Minimizing reduces the app to ~1 Hz logic with no rendering, so it
  stays responsive to session commands.
- GUI private working set measured at 51.1 MB (v0.2.0; 43.6 MB before the bundled fonts) on an NVIDIA OpenGL system, above the
  40 MB target. Most of it is the GPU driver. The wgpu renderer was tried and was worse
  (138 MB default, 310 MB DX12 low-power).
- Unsigned executable: SmartScreen may warn.

## YouTube songs

- Precision is about +-50 ms, limited by the browser's player (a seek fixes drift above
  60 ms, at most every 3 s). A .wav is much tighter.
- Ads cannot be detected reliably and break sync until they end. YouTube Premium avoids them.
- The browser tab must stay open on both PCs and needs one click for sound. Closing it stops
  the video. The OBS Browser Source is a separate player on the same timeline.
- The video length and title are not known to DuetSync: only elapsed time is shown, and
  seeking from the play bar is off.
- Some videos refuse embedding (the page then shows YouTube's error).

## Auto-sync while singing

- It needs both of you to sing (voiced sound on both paths). Quiet passages, talking, and
  heavy voice processing such as Krisp can make the envelopes disagree; DuetSync then
  rejects the estimate and keeps the current offsets.
- It only updates OBS when the new delay differs by more than 30 ms twice in a row, and at
  most every 10 s. Each update can cause a tiny audio blip.
- It never uses the network ping: the delay comes only from the matched singing envelopes.
- It runs only while a song plays, and needs OBS connected with offsets already applied once.
