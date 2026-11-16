# DuetSync progress (session recovery)

## Status: MVP implemented (2026-11-17). Real two-computer testing PENDING.

Gate: `cargo fmt --check` clean, `cargo clippy --all-targets` 0 warnings, `cargo test` 46/46.
Release: `target\release\DuetSync.exe` (5,245,952 bytes); packaged in
`dist\DuetSync-0.1.0-win64\` and `dist\DuetSync-0.1.0-win64.zip`.

## Done
- Core: calibration DSP, playback core (scheduled start, drift correction), WASAPI capture/render,
  timestamp re-gridding for silent devices, encrypted host/guest session, clock sync, OBS client,
  compensation ledger, config, diagnostics.
- `src/app.rs` controller + `src/ui/` egui GUI (tick runs in `App::logic`, so it also works minimized).
- Hardware: `--selftest`, `--cable-test`, `--e2e [--no-ref]` all PASS on this PC (VB-CABLE).
- Docs: README, docs/{ARCHITECTURE,OBS_ROUTING,TROUBLESHOOTING,PERFORMANCE,TEST_REPORT,LIMITATIONS}.md,
  LICENSE, CHANGELOG, THIRD_PARTY_NOTICES, CLAUDE.md, AGENTS.md, DuetSync.example.json.

## Open
- Real two-computer checklist (docs/TEST_REPORT.md).
- GUI private working set 44–49 MB > 40 MB target (NVIDIA OpenGL driver).
- No tray icon; WAV only; LAN/VPN only.

## Gotchas
- PowerShell 5.1 `Get-Content -Raw` corrupts UTF-8: use Edit tool or `[IO.File]` + UTF8.
- Loopback/virtual capture devices send no packets during silence; keep `audio::regrid`.
- eframe 0.37: `App::ui` is skipped while minimized; periodic work belongs in `App::logic`.
