# Changelog

## 0.1.0 (2026-11-17), first MVP

### Added
- Host / guest modes in one portable executable.
- Encrypted LAN/VPN session: 128-bit join codes with expiry, mutual HMAC authentication, ChaCha20-Poly1305 frames, heartbeats, auto-reconnect, version check.
- NTP-style clock sync with drift estimate and worst-case uncertainty.
- Assisted latency calibration with chirp test signals and normalized cross-correlation, plus robust statistics, confidence, rejection of invalid measurements, and an optional guest reference input.
- WASAPI event-driven capture (microphone, output loopback, Discord process loopback) and render with presentation-time scheduling.
- Synchronized local WAV playback: SHA-256 file verification, countdown, pause/stop/seek, gradual drift correction, resync.
- OBS WebSocket 5: input discovery, sync offsets with read-back verification, original-offset ledger (no stacking), restore, manual Render Delay video filter, routing warnings.
- egui GUI (dark charcoal / lavender), diagnostics export, CPU/RAM display.
- `--selftest`, `--bench`, `--cable-test`, `--e2e` hardware checks; `--print-default-config`.

### Known gaps
- Real two-computer testing pending.
- No system tray; WAV only; LAN/VPN only.