# Tasks

Owner key: **CX** = OpenAI Codex CLI (`codex exec`, via `tools/run-codex.ps1`),
**OP** = Claude Opus (lead), **SN** = Claude Sonnet subagent, **HK** = `light-worker`
subagent (cheap model). One writer per file set at a time. Details per run: AGENT_LOG.md.

| ID | Owner | Task | Status |
| --- | --- | --- | --- |
| T0 | OP | Codex availability check, git baseline, task files | done |
| T1 | CX gpt-reserve | Adversarial audit (read-only): 6 findings, all verified | done |
| T2 | CX gpt-5.6-terra | GUI memory < 40 MB | blocked in sandbox; OP continued (wgpu worse); target still missed |
| T3 | CX gpt-5.6-terra | Pre-auth cap, resumable auth, bounded late-start re-seek | done (+ OP integration fixes) |
| T4 | SN | OBS partial-apply safety | done |
| T5 | HK | Docs, changelog, dist 0.1.1 | doing |
| T6 | OP | Gate: fmt, clippy, 52 tests, release, selftest/e2e, perf | done |

## Open
- Real two-computer checklist (docs/TEST_REPORT.md). Needs a second PC and a partner.
- Private working set 43.6 MB vs the 40 MB target (OpenGL driver). Candidates: software
  renderer, or running on an integrated GPU.
- Not implemented: tray icon, non-WAV formats, NAT traversal, automatic video calibration.
