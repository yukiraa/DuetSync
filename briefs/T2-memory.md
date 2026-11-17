# T2: reduce GUI memory (implementation task)

Repo root = cwd. Read `CLAUDE.md` and `docs/PERFORMANCE.md` first.

## Objective
The release GUI (`target\release\DuetSync.exe`, eframe 0.37 + glow/OpenGL) idles at a
**private working set of 44–49 MB**. The target is **< 40 MB**. The DuetSync core uses < 8 MB.
Most of the footprint is the OpenGL driver: on this PC the context lands on NVIDIA, and
`nvoglv64.dll` maps 40.8 MB. Find and implement the smallest change that brings the private
working set below 40 MB **without requiring a dedicated GPU**. Keep idle CPU < 0.2 %.

Ideas to evaluate (measure, don't guess): eframe `NativeOptions` (multisampling, depth/stencil
buffers, `hardware_acceleration`, renderer), font atlas / texture options, preferring a
low-power/integrated adapter, or switching the eframe renderer feature (e.g. wgpu with a
DX12 low-power adapter) only if it measurably wins and the exe stays < 15 MB.

## Files you own (modify ONLY these)
- src/ui/mod.rs, src/ui/theme.rs
- Cargo.toml (eframe dependency/features only)

## Constraints
- No dedicated-GPU requirement. Must still start on a PC with only integrated graphics.
- No allocation or behaviour changes outside the owned files.
- `cargo fmt --check`, `cargo clippy --all-targets` (0 warnings), `cargo test` must pass.
- `cargo build --release` must succeed; exe < 15 MB.

## Measuring
`powershell -NoProfile -ExecutionPolicy Bypass -File tools\perf.ps1 -Exe target\release\DuetSync.exe -Secs 30`
prints CPU and the "private working set" for the visible and minimized states. If the
sandbox prevents launching the GUI or reading counters, say so explicitly: do NOT invent
numbers. The lead will re-measure.

## Final message (max ~300 words)
What you changed and why, the exact commands you ran with the measured before/after
numbers (or "could not measure: reason"), and the test and clippy results.
