# AGENTS.md

Same rules as [CLAUDE.md](CLAUDE.md). Summary:

- Build/test gate: `cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), `cargo test`.
- Release: `cargo build --release` → `target\release\DuetSync.exe`.
- Real-hardware checks: `DuetSync.exe --selftest`, `--selftest --e2e` (needs VB-CABLE).
- No allocation, locks or I/O in audio callbacks. Never invent benchmark or test results.
- Architecture: `docs/ARCHITECTURE.md`. Status: `PROGRESS.md`.
