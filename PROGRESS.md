# DuetSync progress (session recovery)

## Status: v0.4.0 (2026-11-17). Real two-computer testing PENDING.

Gate: `cargo fmt --check` clean, `cargo clippy --all-targets` 0 warnings, `cargo test` 79/79.
Release: packaged in `dist\DuetSync-0.4.0-win64(.zip)`.
Agent routing: Opus only (user request); no Codex, no subagents.

## Internet mode (0.4.0)
- `src/network/internet/`: stun.rs (public addr), signal.rs (ntfy.sh topic from HMAC of secret,
  records sealed ChaCha20-Poly1305, subscribe `since=all`), https.rs (WinHTTP; call
  QueryDataAvailable before ReadData), tunnel.rs (HMAC-tagged UDP, reliable stream, epochs,
  event-driven pump with TCP reader thread), mod.rs (InternetHost / InternetGuest).
- Guest session connects to the local bridge (127.0.0.1) via `Guest::set_extra`; session code
  unchanged. Invite = `hosts#code`; code-only works over the internet.
- Router here has UPnP off (`--upnp-probe`); UPnP code is probe-only.
- Untested: two different home networks, symmetric NAT / CGNAT.
- Checks: `--selftest --e2e --internet`, `--selftest --relay-test`.
Git repo (local only, branch `main`). Coordination: TASKS.md, AGENT_LOG.md, briefs/.

## Multi-agent setup
- Codex: Codex app's CLI `%LOCALAPPDATA%\OpenAI\Codex\bin\*\codex.exe` (0.167.0-alpha.3.2,
  ChatGPT login). Claude Code Codex plugin NOT installed; run via `tools/run-codex.ps1`.
  Working models: `gpt-reserve` (audit), `gpt-5.6-terra` (T2-T8 implementation).
  `gpt-6-astra` is rejected for ChatGPT accounts. As of 2026-11-17 17:5x the catalog also
  lists, and smoke tests pass for, `gpt-6.1-sol`, `gpt-6-sol`, `gpt-5.6-sol` and
  `gpt-6.1-terra`; earlier the same day the catalog showed only 5 models. Use `gpt-6.1-sol`
  for primary implementation from now on. The Codex sandbox has no crates.io access and can't
  measure the GUI.
- Claude subagents: Sonnet (`general-purpose`, model sonnet), `light-worker` (mechanical).

## Hardware results on this PC
selftest PASS; E2E PASS (playback worst 0.83 ms, re-anchored start 0.76 ms);
idle CPU 0.021 % visible / 0.005 % minimized; private WS 43.6 MB (target 40 MB missed).

## Gotchas
- PowerShell 5.1 `Get-Content -Raw` corrupts UTF-8: use Edit or `[IO.File]` + UTF8.
- Loopback/virtual capture sends no packets during silence; keep `audio::regrid`.
- eframe 0.37: `ui` is skipped while minimized; periodic work lives in `App::logic`.
- PlayInfo has protocol `gen` and local ring `local_gen`; Retime uses `local_gen`.
