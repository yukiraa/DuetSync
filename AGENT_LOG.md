# Agent log

Only real invocations are recorded. Token counts are copied from the provider's own output
when it prints them; otherwise "not exposed".

| Task | Provider | Model | Responsibility | Files modified | Status | Tests | Tokens | Blockers |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| T0-a | OpenAI Codex CLI 0.167.0-alpha.3.2 (`codex exec`, read-only, ephemeral) | gpt-6-astra | smoke test | none | FAILED: "model is not supported when using Codex with a ChatGPT account" | n/a | not exposed | model unavailable on this account |
| T0-b | OpenAI Codex CLI | gpt-5.6-terra | smoke test | none | ok (`CODEX_OK`) | n/a | printed (not captured) | none |
| T0-c | OpenAI Codex CLI | gpt-reserve | smoke test | none | ok (`CODEX_OK`) | n/a | 4,830 | none |
| T1 | OpenAI Codex CLI (`codex exec`, read-only) | gpt-reserve, effort high | adversarial audit | none | done, 9 min; 6 findings (2 high, 4 medium), all verified valid by Opus | `cargo fmt --check` pass; test/clippy blocked by a build lock held by T2 | 181,196 | none |
| T2 | OpenAI Codex CLI (`codex exec`, workspace-write) | gpt-5.6-terra, effort high | GUI memory < 40 MB | none kept (restored) | blocked, 10.6 min: sandbox has no crates.io access (wgpu path) and no GUI perf counters; glow software mode exits at startup | fmt/clippy/46 tests/release pass on restored tree | 96,404 | sandbox network + GUI measurement |
| T2b | Claude (this session) | Opus 5.5 | T2 continuation outside the sandbox: wgpu renderer experiment in worktree `exp/wgpu` (discarded) | none kept | done, rejected: wgpu (default) 137.9 MB private WS, wgpu + DX12 low-power 309.8 MB, exe 8.4 MB; glow (44 MB) stays | measured with tools/perf.ps1 | not exposed | GL driver footprint is the floor with glow |
| T3 | OpenAI Codex CLI (`codex exec`, workspace-write) | gpt-5.6-terra, effort high | audit fixes: pre-auth handshake cap, resumable auth (expiry + rotation), bounded late-start skip + `needs_reseek` | src/network/session.rs, src/playback/core.rs, src/app.rs (start_local), tests/network.rs | done, 10.7 min | Codex: fmt, clippy -D warnings, 50 tests pass | 104,566 | none |
| T3-int | Claude (this session) | Opus 5.5 | review of T3: fixed re-anchor vs Retime anchor mismatch (separate local ring generation), wired `needs_reseek` render to app, added E2E re-anchor round | src/app.rs, src/audio/wasapi.rs, src/selftest.rs | done | 50 tests; hardware E2E PASS, re-anchored start worst 1.20 ms | not exposed | none |
| T0-d | Claude (this session) | Opus 5.5 | availability check, git init, task files | .gitignore, TASKS.md, AGENT_LOG.md, tools/* | done | n/a | not exposed | Claude Code Codex plugin (`openai/codex-plugin-cc`) not installed, so the CLI fallback is used |

Earlier history (before this log): the 0.1.0 MVP baseline (commit `02e1472`) was written
by Claude Opus in this session, with one `light-worker` subagent for LICENSE, CHANGELOG
and dist packaging. Codex was not used for it.
