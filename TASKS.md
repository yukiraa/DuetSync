# Tasks

Owner key: **CX** = OpenAI Codex CLI (`codex exec`), **OP** = Claude Opus (lead),
**SN** = Claude Sonnet subagent, **HK** = Claude Haiku-class subagent (`light-worker`).
Only one writer per file set at a time. Status: todo / doing / done / blocked.

| ID | Owner | Task | Files owned | Status |
| --- | --- | --- | --- | --- |
| T0 | OP | Codex availability check, git baseline, task files | TASKS.md, AGENT_LOG.md, .gitignore | done |
| T1 | CX (gpt-reserve) | Adversarial audit: timing, DSP, clock sync, audio-thread safety, races, OBS restore, auth (read-only) | none (report only) | doing |
| T2 | CX (gpt-5.6-terra) | GUI private working set < 40 MB without a dedicated-GPU requirement; keep idle CPU < 0.2 % | src/ui/mod.rs, src/ui/theme.rs, Cargo.toml | doing |
| T3 | CX | Fix verified critical/high findings from T1 (systems files) | assigned after triage | todo |
| T4 | SN | Fix verified app/OBS/UI findings from T1; tray icon if cheap | src/app.rs, src/ui/panels.rs, src/obs/* | todo |
| T5 | HK | Update TEST_REPORT, PERFORMANCE, CHANGELOG, dist package | docs/*, CHANGELOG.md, dist/ | todo |
| T6 | OP | Integration gate: fmt, clippy, test, release, selftest/e2e, perf | none | todo |

Out of scope (needs hardware/people): real two-computer checklist in docs/TEST_REPORT.md.
