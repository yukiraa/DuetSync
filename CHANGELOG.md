# Changelog

## 0.3.1 (2026-11-17), joining fixes

### Fixed
- A joining guest no longer waits silently. The join screen shows the address being tried,
  how long, how many tries failed, and the real reason in plain words (no answer / refused /
  unreachable / closed), plus a checklist after 2 failed tries.
- Retries happen every 2 s at most (was up to 5 s).
- A fresh join is ready in about 1 s (was about 2 s): the first clock-sync pings are sent every 100 ms.
- The host's anti-flood limit counted live connections as "pending", so stale links could
  block new joins. It now counts only unfinished handshakes.
- The host's Step 1 shows "Partner can't join?":
  - why someone was refused (version, expired, wrong code);
  - a warning when Windows marks the network "Public";
  - a Tailscale hint when the invite only works on the same Wi-Fi.
- Refused guests see the reason on the join screen.

## 0.3.0 (2026-11-17), YouTube songs and live auto-sync

### Added
- Paste a YouTube link instead of a .wav. Each performer's browser plays it with YouTube's official player, and the host adds the same page to OBS as a Browser Source. DuetSync starts both on the shared clock and keeps them aligned.
- Live auto-sync while singing: DuetSync compares the guest's real voice on their PC with what arrives through Discord, and updates the OBS delay when it changes by more than 30 ms (network ping is never used).
- `--selftest --video-test <link>`: real-browser YouTube sync check.

### Changed
- Session protocol v2: 0.3 and older versions refuse each other with a clear message.

### Measured (one PC, real browser, real YouTube embed)
- Video alignment after the seek-lead fix: median 23.7 ms, worst 24.6 ms, no buffering stalls (was 118.5 / 171 ms).

## 0.2.0 (2026-11-17), friendly guided UI

### Changed
- New look: cream background, sticker-style cards and pill buttons, Fredoka and Nunito fonts (OFL, bundled).
- First launch asks "Who's singing today?" (host or guest).
- The tabs are replaced by a step-by-step journey: host 6 steps, guest 5. Done steps get a star, and the current step is highlighted.
- A mascot (coral host, mint guest) always says what to do next, with one main button per screen.
- Rare options are under "+ Advanced settings".
- "Go to step N" appears only when an unfinished earlier step blocks the current one.
- Setup tests now mark their step done only when sound was actually heard.

### Known
- GUI private working set is 51.1 MB on the benchmark PC (was 43.6 MB): the bundled fonts enlarge the glyph atlas.

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
