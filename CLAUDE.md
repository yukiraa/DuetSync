# DuetSync: agent notes

Rust, Windows-only desktop app (egui/eframe + glow, WASAPI via `windows` crate). Single crate:
logic in `src/lib.rs` modules, GUI in `src/ui`, controller in `src/app.rs`.

## Commands
- `cargo fmt --check && cargo clippy --all-targets && cargo test`, all must be clean.
- `cargo build --release` → `target\release\DuetSync.exe`
- Hardware checks: `DuetSync.exe --selftest [--bench N] [--cable-test] [--e2e [--no-ref]]`
  (`--e2e`/`--cable-test` need VB-CABLE; release builds print to the parent console).

## Rules
- Never fabricate measurements; docs cite only numbers produced by the commands above.
- Audio threads (`audio/wasapi.rs` loops, `playback/core.rs::fill`): no allocation, no blocking
  locks, no I/O.
- Captures must stay timestamp-regridded (`audio::regrid`): silent devices deliver no packets.
- OBS offsets go through `obs::compensation::plan` + ledger (never stack compensation).
- Keep secrets (join code, OBS password) out of logs and diagnostics.
- On Windows PowerShell 5.1, do not rewrite UTF-8 sources with `Get-Content`/`Set-Content`.
  Use the Edit tool or `[IO.File]` with UTF8.
- `PROGRESS.md` = session recovery notes. Keep it current.

## Testing (user preference)
- YouTube tests (`--selftest --video-test <link>`): use videos shorter than 3 minutes only.

## Agent routing (user preference)
- Codex first: implementation, reviews, docs and mechanical edits go to `tools/run-codex.ps1` (`gpt-6.1-sol`; `gpt-5.6-luna` for trivial jobs). Claude only orchestrates and verifies, tersely. Claude subagents only when the Codex sandbox cannot do the job (network, GUI measurement).
