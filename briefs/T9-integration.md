# T9: wire YouTube video + live auto-sync into the app and UI (Claude Sonnet)

Repo: `D:\+ Personal Project\21. Karaoke delay`. Read `CLAUDE.md`, then the new modules built by
Codex: `src/live/*` (live delay tracker, brief `briefs/T7-live-sync.md`) and `src/video/*` +
`assets/video/player.html` (YouTube bridge, brief `briefs/T8-video-bridge.md`). Use their
ACTUAL public APIs; read the code. Do not modify `src/live/*`, `src/video/*` or
`src/audio/wasapi.rs`. If an API is missing, add it to `app.rs` around theirs or report back.

## Files you own
`src/network/protocol.rs`, `src/app.rs`, `src/config.rs`, `src/ui/*`, `src/selftest.rs`
(optional e2e additions), `tests/network.rs` (only if the protocol change needs it), `README.md`
(short new sections), `docs/OBS_ROUTING.md` (YouTube section), `docs/LIMITATIONS.md` (new limits).

## 1. Protocol (`protocol.rs`)
- Bump `PROTO_VERSION` to 2 (old and new peers must refuse each other cleanly; that already
  works).
- Add `Msg::Media(Option<MediaInfo>)` with `MediaInfo::Wav(TrackInfo) | MediaInfo::YouTube {
  video_id: String }`. Keep `Msg::Track`, or replace it, but keep the WAV SHA-256 check
  semantics.
- Add `Msg::MediaStatus { kind: String, id_or_sha: Option<String> }` if needed for matching.
- Add `Msg::VoiceEnvelope { start_host_ms: i64, bins: Vec<u8> }` (guest to host, about 1 KB/s;
  stay under `MAX_FRAME`).

## 2. App (`app.rs`)
**Media source.**
- The host either loads a WAV (as today) or sets a YouTube link: `set_youtube(link)` uses
  `video::parse_youtube` and reports an error for an invalid link.
- In YouTube mode the host starts a `VideoBridge` (fixed port 47811, falling back to 0 if busy),
  calls `load(id)`, and sends `Msg::Media(YouTube)` to the guest.
- The guest auto-starts its own bridge, loads the same id, and opens the player page in the
  default browser (`ShellExecuteW` "open" via the `windows` crate; add the feature
  `Win32_UI_Shell` to Cargo.toml if needed). The host gets an "Open video" button that does the
  same.
- Match = same video id.

**Playback routing.**
- `host_play`, `pause`, `stop`, `seek`, `resync`, `guest_play` and the Pause/Stop handlers drive
  the `VideoBridge` (`play_at(start_local_us, from_s)` etc.) in YouTube mode, and the WAV engine
  otherwise.
- Same countdown and the same shared-clock anchor (`PlayInfo`).
- Positions in seconds for video; keep frames for WAV.
- `GuestStatus` reports the video position (seconds × 48000 as frames, or add a field) so the
  host's "In time!" badge works in both modes.
- Track length for YouTube is unknown unless the bridge reports it: show the elapsed time only.

**Live auto-sync** (user choice: measure from singing, auto re-apply).
- `cfg.auto_sync: bool` (default true).
- While playing (and auto_sync is on):
  - Guest: run `LiveCapture` on its mic (`cfg.guest_reference_input`). Every ~1 s, convert the
    finished envelope bins to the host clock (`est.to_host`) and send `Msg::VoiceEnvelope`.
  - Host: run `LiveCapture` on `guest_capture_source()`. Feed both series into
    `LiveTracker`; every 2 s call `estimate`; pass accepted ones to `LiveFilter`.
  - When `should_reapply(applied_guest_latency, now)` returns a value: set it as the live
    guest latency, run the existing `plan()` and send `ObsCmd::SetOffsets` (no confirm modal;
    the user chose auto). Record the ledger the same way `confirm_apply` does (record BEFORE
    sending). Log "Auto-sync: 182 -> 221 ms, OBS updated".
- Do this only when OBS is connected and DuetSync has applied offsets before (the ledger is
  non-empty). Otherwise only show the live number.
- Stop the captures when playback stops. Never run them when idle.
- Expose for the UI: `live_latency_ms: Option<f64>`, `live_updated_us`,
  `last_auto_apply: Option<(f64, f64, i64)>` (from, to, when).

## 3. UI (`src/ui/*`, keep the fun style and widgets)
**Load the song step (host):** two choices as big sticker toggles, "WAV file" and "YouTube
link".
- YouTube: a paste field + Coral "Use this video".
- Then a card with the video id, Coral "Open video" (opens the player page), and
  "For your stream: add this as an OBS Browser Source" with the URL (`bridge.url("obs")`), a
  White "Copy" button and the hint "In OBS: Sources, +, Browser, paste the URL, size 1920x1080,
  tick Control audio via OBS".
- The guest's Load step shows "Your host picked a YouTube video" + Mint "Open video" and a
  check row for "Video page connected" (bridge client with role player present).
- The match check rows work for both modes.

**Send to OBS step:** in YouTube mode, the "Instrumental" role hint says "pick the DuetSync
Browser Source". Wording only.

**Sing step:**
- The stage shows the video title or id.
- A small sticker row: "Auto-sync" toggle (on), "Live delay: 205 ms" (or "listening..." /
  "not enough singing yet"), and "Last auto-update: 182 -> 221 ms, 12 s ago".
- The video page client status ("Video in sync: +12 ms" from `clients()`), sun-coloured when
  |err| > 60 ms.
- Keep ASCII text (no arrows: write "->").

**Sound unlock (required):** browsers block sound until the page is clicked once.
`ClientInfo.needs_click` is true until that click. Wherever the video status shows (Load step,
Sing step), when any `player` client needs a click, show a sun `check_row`: "Click the video
tab once to turn on sound". Mention it in the README YouTube section.

## 4. Docs (short)
- README: a "YouTube songs" section (how to paste, open, add OBS Browser Source; ±50 ms; ads
  break sync until they end; Premium avoids ads) and an "Auto-sync while singing" section
  (what it measures, the 30 ms threshold, a tiny blip per update, needs both to sing).
- OBS_ROUTING: Browser Source as the Instrumental source in YouTube mode.
- LIMITATIONS: YouTube precision and ads, the browser must stay open, and auto-sync needs
  singing, could be confused by Krisp, and never uses ping.

## Gate
`cargo fmt --check`, `cargo clippy --all-targets` 0 warnings, `cargo test` all pass,
`cargo build --release`. Keep idle CPU unchanged: no bridge or capture threads unless media is
YouTube or playing. Do not commit. Final report ≤ 250 words: files, the new protocol messages,
how auto-sync decides, gate results.
