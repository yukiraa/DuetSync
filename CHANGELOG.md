# Changelog

## 0.1.1 (2026-11-17), audit fixes

Independent audit by OpenAI Codex (gpt-reserve); fixes by Codex (gpt-5.6-terra), Claude Sonnet and Claude Opus. See AGENT_LOG.md.

### Fixed
- Host caps concurrent unauthenticated handshakes at 4 (LAN connection flooding).
- Join codes now really expire: after the first join, the guest reconnects with a per-session resume credential. New joins with an expired or rotated code are rejected, and "New code" no longer breaks reconnects.
- Late or large re-sync skips no longer drain the 2 s buffer while still reporting a sync position. The player re-seeks on the shared timeline instead.
- A short countdown or a late Play message re-anchors the start 500 ms ahead on the same musical timeline.
- OBS: original offsets are recorded before writing. A connection drop mid-apply can no longer orphan a change or stack compensation, and every input's result is reported.
- The invite "Copy" button now works (eframe clipboard feature).
- Session shutdown race that could leave a link running.

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