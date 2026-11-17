# T8: local YouTube player bridge (implementation)

Repo root = cwd. Read `CLAUDE.md` first. Rust, Windows. Use `$env:CARGO_TARGET_DIR="target-t8"`
for every cargo command (another agent builds in parallel). No new crates (no network in the
sandbox). `tungstenite` 0.30 (feature `handshake`) is available for the server side.

## Why
Users paste a YouTube link and both performers should see and hear the video in sync. DuetSync
has no built-in browser (hard rule), and downloading from YouTube breaks YouTube's terms. So
DuetSync serves a tiny page on **127.0.0.1 only** that embeds **YouTube's official IFrame
Player API**:
- each performer opens it in their normal browser;
- the host also adds it to OBS as a Browser Source (role `obs`).

DuetSync controls every page over a local WebSocket and keeps it on the shared timeline.

## Files you own (modify ONLY these)
- `src/video/mod.rs` (stub exists) plus new files under `src/video/`
- `assets/video/player.html` (new; embedded with `include_str!`)
- `tests/video.rs` (new)

## Build
1. **`video/mod.rs`:** `parse_youtube(input) -> Option<String>` returns the 11-char id
   (`[A-Za-z0-9_-]{11}`).
   - Accept `youtube.com/watch?v=`, `m.`/`music.` hosts, `youtu.be/<id>`, `/shorts/<id>`,
     `/embed/<id>`, `/live/<id>`, extra query params, and a bare id.
   - Reject everything else.
2. **`video/server.rs`:** `VideoBridge::start(port: u16) -> Result<VideoBridge, String>`.
   - Binds `127.0.0.1:port` only (0 = any; report the bound port).
   - One accept thread. A tiny HTTP/1.1 handler: `GET /` and `GET /?role=obs` return the
     embedded page (`text/html; charset=utf-8`, `Cache-Control: no-store`); `GET /ws?role=player|obs`
     upgrades to a WebSocket (`tungstenite::accept_hdr`); anything else returns 404.
   - Reject when the `Host` header is not `127.0.0.1:<port>`/`localhost:<port>`, or when an
     `Origin` is present and is not the same origin.
   - At most 4 clients; requests capped at 8 KB; each message ≤ 8 KB.
   - One thread per client; drop on Drop.
3. **Clock mapping per client (NTP-style over localhost):**
   - Every 1 s the app sends `{"t":"ping","id":n,"t1":<app_us>}`; the page answers
     `{"t":"pong","id":n,"t1":..,"t2":<page_ms>}`, where `page_ms = performance.timeOrigin +
     performance.now()`.
   - Keep the min-RTT sample of the last 20 → `offset` (page_ms ↔ app local µs) and
     `rtt`. Use `crate::clock::now_us()` as the app clock.
4. **Commands (app → page JSON):**
   - `{"t":"load","videoId":".."}`
   - `{"t":"cue","pos_s":x}` (seek + pause, preroll)
   - `{"t":"play_at","page_ms":X,"from_s":Y}`
   - `{"t":"pause"}`, `{"t":"seek","pos_s":x}`, `{"t":"stop"}`
   - Page → app: `{"t":"ready"}`, `{"t":"pong",..}`, `{"t":"error","code":n}`, and
     `{"t":"state","state":"unstarted|ended|playing|paused|buffering|cued|ad","pos_s":p,"page_ms":m}`
     (on every change and every 250 ms while playing).
5. **Public API:**
   - `load(id)`, `play_at(start_local_us, from_s)` (cue first if the start is > 1.5 s away,
     then schedule), `pause()`, `stop()`, `seek(pos_s)`;
   - `clients() -> Vec<ClientInfo { role, state, last_pos_s, error_ms: Option<f64>, rtt_ms }>`;
   - `position_at(local_us) -> Option<f64>` (from the first `player` client, extrapolated
     while playing);
   - `url(role) -> String`;
   - `port()`.
6. **Drift correction:** a thread every 500 ms, for each `playing` client.
   - `desired = from_s + (now - start)/1e6`; `err = reported pos extrapolated to now - desired`.
   - If |err| > 60 ms and the state is not `buffering`/`ad`: send `seek` to `desired`; then a
     3 s cooldown for that client.
   - Expose `error_ms` for the UI.
   - Unit-test the policy as a pure function.
7. **`assets/video/player.html`:** a single page. Inline CSS/JS; the only external script is
   `https://www.youtube.com/iframe_api`.
   - The player fills the window on black; `controls=0`, `disablekb=1`, `playsinline=1`,
     `rel=0`, `modestbranding=1`.
   - Role `player` shows a small overlay at the bottom: "DuetSync video - keep this tab
     open. This tab's volume is what you hear." It fades after 5 s. Role `obs` shows no overlay.
   - Connects to `ws://<location.host>/ws?role=..` and reconnects every 2 s when closed.
   - Implements the commands above. `play_at`: `setTimeout` to the target time, then
     `playVideo()` (no busy-wait).
   - Maps YT states to the strings above; reports an ad when `getVideoData().title` is empty
     while playing (best effort; comment that ads cannot be reliably detected).
   - No `eval`; escape nothing into innerHTML.

## Tests (`tests/video.rs` plus unit tests; no internet)
- `parse_youtube`: at least 10 valid and 6 invalid forms.
- HTTP: `GET /` returns 200 with the page; `GET /nope` returns 404; a foreign `Host` and a
  foreign `Origin` are rejected.
- A fake page client (a tungstenite client in the test) handshakes and answers pings with a
  page clock offset by +12 345 ms → the bridge's offset estimate is within 2 ms.
  `play_at(now + 2 s, 30.0)` sends `cue` then `play_at` with the correctly mapped `page_ms`
  (±2 ms).
- Drift policy: a fake client reporting +200 ms error gets a `seek`; +20 ms does not; no second
  seek within the cooldown.
- A 5th client is refused; Drop closes the server.

## Gate
`cargo fmt --check`, `cargo clippy --all-targets` 0 warnings, `cargo test` all pass (with
`CARGO_TARGET_DIR=target-t8`).

## Final message (max 250 words)
Files and public API, the URL format, the test list with results, and the known limits
(precision, ads).
