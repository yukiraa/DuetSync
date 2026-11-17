# Performance report

All numbers were **measured** on one machine on 2026-11-17 with the release build
(`opt-level="s"`, LTO, 1 codegen unit, `panic=abort`, stripped).

## Benchmark machine

| Item | Value |
| --- | --- |
| CPU | Intel Core Ultra 5 225 (10 cores / 10 threads) |
| RAM | 31.5 GB |
| GPU | Intel Graphics + NVIDIA GeForce RTX 5060 (OpenGL context created on NVIDIA, `nvoglv64.dll`) |
| OS | Windows 11 Pro, build 26200 |
| Toolchain | rustc 1.99.1 (x86_64-pc-windows-msvc) |
| Background | Discord and other desktop apps running; OBS not running |

This is a mid/high-end desktop. Low-end PCs were **not** measured.

## Results

| Metric | Target | Measured | Method |
| --- | --- | --- | --- |
| Executable size | < 15 MB | **5.7 MB** (5,731,840 bytes, v0.2.0) | file size of `target\release\DuetSync.exe` |
| GUI idle CPU, window visible | < 0.2 % | **0.031 %** of all cores (0.31 % of one core), v0.2.0; v0.1.1: 0.021 % | `Process.TotalProcessorTime` delta over 45 s after 8 s warm-up, ÷ wall time ÷ 10 cores |
| GUI idle CPU, minimized | < 0.2 % | **0.005 %** of all cores (v0.1.0: 0.003 %) | same, after `ShowWindow(SW_MINIMIZE)` |
| Active playback CPU (two controllers + two output streams, headless) | < 1 % | **0.04–0.11 %** of all cores | `--selftest --e2e`, `GetProcessTimes` during ~10 s of playback; one process runs host **and** guest |
| Output stream open, idle (headless) | — | below resolution (reported 0.000 % over 30 s) | `--selftest --bench 30` |
| GUI private working set | < 40 MB | **51.1 MB** v0.2.0 (43.6 MB v0.1.1; target **missed**) | perf counter `Working Set - Private` |
| GUI working set / private bytes | — | 76–81 MB / 80–87 MB | `Process.WorkingSet64` / `PrivateMemorySize64` |
| Headless core private bytes | — | 4.9 MB (one output stream), 7.6 MB (host+guest playing) | `K32GetProcessMemoryInfo` |

### Why the memory target is missed

The DuetSync core (audio, network, DSP, OBS) uses under 8 MB. The remainder of the GUI's
footprint belongs to the OpenGL stack: the NVIDIA OpenGL driver module alone maps
**40.8 MB** (`nvoglv64.dll`) plus its own allocations. On a PC where the window runs on an
integrated GPU, the number will be different (not measured). Possible future work: a
software or DirectX renderer, or a smaller font atlas.

## What keeps CPU low

* All audio threads are event-driven (WASAPI event callbacks, MMCSS "Pro Audio"). No
  polling or busy-waiting; no allocation or blocking locks in the render/capture loops.
* Lock-free bounded SPSC rings between audio and worker threads.
* Instrumental streamed from disk with a ~2 s ring. Memory does not grow with song length.
* FFT correlation runs only after a calibration, on a short-lived worker thread.
* Capture streams exist only while calibrating, testing, or showing a level meter.
* The UI repaints reactively: 2 Hz when idle, 15 Hz while something moves, ~1 Hz logic-only
  when minimized (no rendering).

## Not measured

* Low-end PCs, laptops on battery, integrated-GPU-only systems.
* CPU while OBS, Discord, VTube Studio and a game all run at full load.
* Two-computer sessions (see the test report).

## Renderer experiment (v0.1.1)

glow (OpenGL) was compared with eframe's wgpu renderer in a throwaway git worktree,
measured with `tools/perf.ps1`:

| Renderer | Private working set | Exe |
| --- | --- | --- |
| glow (shipped) | 43.6 MB | 5.3 MB |
| wgpu (default backend) | 137.9 MB | 8.4 MB |
| wgpu, `WGPU_BACKEND=dx12`, `WGPU_POWER_PREF=low` | 309.8 MB | 8.4 MB |

glow remains the lightest option measured. A Codex attempt (T2) could not measure inside
its sandbox, so these numbers come from the lead outside the sandbox.

## v0.3.0 note

YouTube mode adds a local web page (in the user's own browser) and a tiny 127.0.0.1 server
in DuetSync. Live auto-sync adds one capture stream on each PC, only while a song plays.
Neither runs while idle. Not re-measured for CPU/RAM in this release.
