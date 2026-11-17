# Known limitations

**Not production-proven.** Real two-computer testing is still pending
(see [TEST_REPORT.md](TEST_REPORT.md)).

## Latency and Discord

- DuetSync does not remove latency. Performers still hear each other late in Discord.
- Compensation is **static**. Discord's jitter buffer changes its delay during a song and
  DuetSync cannot observe that. The safety margin absorbs small changes but does not track
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

- LAN or private VPN only: no NAT traversal, relay, or room codes. Use Tailscale or similar.
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
- GUI private working set measured at 43.6–49 MB on an NVIDIA OpenGL system, above the
  40 MB target. Most of it is the GPU driver. The wgpu renderer was tried and was worse
  (138 MB default, 310 MB DX12 low-power).
- Unsigned executable: SmartScreen may warn.
