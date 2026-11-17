# T11: release 0.3.0 (docs, version, dist). Mechanical, follow exactly.

Repo root = cwd. Change only the files named here. Write UTF-8 without BOM.

1. `Cargo.toml`: set `version = "0.3.0"`.
2. `CHANGELOG.md`: insert directly under `# Changelog`:
```
## 0.3.0 (2026-11-17), YouTube songs and live auto-sync

### Added
- Paste a YouTube link instead of a .wav. Each performer's browser plays it with YouTube's official player, and the host adds the same page to OBS as a Browser Source. DuetSync starts both on the shared clock and keeps them aligned.
- Live auto-sync while singing: DuetSync compares the guest's real voice on their PC with what arrives through Discord, and updates the OBS delay when it changes by more than 30 ms (network ping is never used).
- `--selftest --video-test <link>`: real-browser YouTube sync check.

### Changed
- Session protocol v2: 0.3 and older versions refuse each other with a clear message.

### Measured (one PC, real browser, real YouTube embed)
- Video alignment after the seek-lead fix: median 23.7 ms, worst 24.6 ms, no buffering stalls (was 118.5 / 171 ms).
```
3. `docs/TEST_REPORT.md`: append at the end:
```

## v0.3.0 additions

Automated: 73 tests pass (`cargo test`), including the live-delay tracker (synthetic singing:
delay errors under 0.1 ms) and the YouTube bridge (URL parsing, Host/Origin checks, clock
mapping, simulated seek-latency convergence).

Real browser, one PC (`DuetSync.exe --selftest --video-test https://www.youtube.com/watch?v=aqz-KE-bpKQ`,
Chromium-based browser, localhost RTT about 0.2 ms):

| Version | Median error | p95 | Worst | Stalls |
| --- | --- | --- | --- | --- |
| before the fix | 118.5 ms | 170.4 ms | 171.0 ms | a seek every 3 s |
| after the fix (learned seek lead) | 23.7 ms | 24.4 ms | 24.6 ms | none |

PENDING (needs two PCs and Discord):
- live auto-sync with real singing over Discord;
- YouTube on two PCs at once;
- OBS Browser Source audio alignment;
- behaviour when ads play.
```
4. `docs/PERFORMANCE.md`: append at the end:
```

## v0.3.0 note

YouTube mode adds a local web page (in the user's own browser) and a tiny 127.0.0.1 server
in DuetSync. Live auto-sync adds one capture stream on each PC, only while a song plays.
Neither runs while idle. Not re-measured for CPU/RAM in this release.
```
5. Run `cargo build --release` and confirm `target\release\DuetSync.exe --version` prints `DuetSync 0.3.0`.
6. Packaging:
   - Delete `dist\DuetSync-0.2.0-win64` and `dist\DuetSync-0.2.0-win64.zip` if present.
   - Create `dist\DuetSync-0.3.0-win64\` containing `target\release\DuetSync.exe`, README.md,
     LICENSE, CHANGELOG.md, THIRD_PARTY_NOTICES.md, DuetSync.example.json, the `docs\` folder,
     and `assets\fonts\` copied as `fonts\`.
   - Zip it with PowerShell `Compress-Archive -Path "dist\DuetSync-0.3.0-win64\*" -DestinationPath "dist\DuetSync-0.3.0-win64.zip" -Force`.

Final message (max 120 words): edits done, exe size in bytes, zip size, `--version` output.
