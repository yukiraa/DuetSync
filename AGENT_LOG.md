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
| T0-d | Claude (this session) | Opus 5.5 | availability check, git init, task files | .gitignore, TASKS.md, AGENT_LOG.md, tools/* | done | n/a | not exposed | Claude Code Codex plugin (`openai/codex-plugin-cc`) not installed, so the CLI fallback is used |

Earlier history (before this log): the 0.1.0 MVP baseline (commit `02e1472`) was written
by Claude Opus in this session, with one `light-worker` subagent for LICENSE, CHANGELOG
and dist packaging. Codex was not used for it.
