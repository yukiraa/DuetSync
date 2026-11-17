# T6: "fun" guided UI in egui (Claude Sonnet)

Repo: `D:\+ Personal Project\21. Karaoke delay`. Read `CLAUDE.md` first. Rust, eframe/egui **0.37**
(glow). The user approved this design. Its pixel reference is the five mockup files in
`docs/ui-mockup/*.dc.html`: plain HTML with inline styles. Read them for exact colours,
sizes and copy. Goal: a first-time user always knows **what to click next**.

## Hard constraints
- Keep ALL existing functionality reachable; keep `src/app.rs` logic as is (only the small
  additions listed below). Panels in `src/ui/panels.rs` show what each action needs.
- No new crates. Fonts are loaded with `include_bytes!` from `assets/fonts/`.
- Keep the repaint policy in `src/ui/mod.rs::logic` (500 ms idle, 66 ms active, 1 s minimized).
  Mascot bobbing animation only while `app.playing()` or calibrating (already "active").
- egui default fonts lack some glyphs (arrows, emoji). Keep egui's default fonts as fallbacks
  after the new fonts, and still prefer plain ASCII text (no arrows or emoji).
- Text contrast: dark ink text on coral/mint buttons (never white text on coral).
- Gate: `cargo fmt --check`, `cargo clippy --all-targets` = 0 warnings, `cargo test` all pass,
  `cargo build --release` ok, exe < 15 MB. On PowerShell 5.1 never rewrite sources with
  Get-Content/Set-Content (UTF-8 corruption); use the Edit/Write tools.

## Files you own
`src/ui/*` (rewrite freely; add e.g. `fun.rs` widgets, `mascot.rs`, `steps.rs`), `assets/fonts/*`,
`src/config.rs` (add `onboarded: bool`, default false), `THIRD_PARTY_NOTICES.md` (fonts section),
and in `src/app.rs` ONLY these additions: `pub guest_source_ok: bool`, `pub routing_ok: bool`
(set from the result of `test_guest_capture` / `test_routing`: add a field recording which
test is running so `poll_workers` knows which flag to set), `pub confirmed_steps: std::collections::BTreeSet<u8>`.

## Fonts (OFL-1.1, free to bundle)
Get static TTFs for **Fredoka 600 + 700** and **Nunito 400 + 800**. Use the Google Fonts CSS API
with an old-browser User-Agent so it returns truetype URLs, e.g.
`curl -A "Mozilla/5.0 (Windows NT 6.1; WOW64; rv:30.0) Gecko/20100101 Firefox/30.0" "https://fonts.googleapis.com/css2?family=Fredoka:wght@600;700&family=Nunito:wght@400;800"`,
then download each `src: url(...)` .ttf into `assets/fonts/` with clear names
(`Fredoka-SemiBold.ttf`, ...). Add the OFL text as `assets/fonts/OFL.txt` (from
`https://raw.githubusercontent.com/google/fonts/main/ofl/fredoka/OFL.txt` and the Nunito one;
one file per family is fine), and a fonts line in THIRD_PARTY_NOTICES.md. If downloading fails,
stop and report; do not substitute other fonts silently.
Register families `FontFamily::Name("fredoka")`, `"fredoka-bold"`, `"nunito"`, `"nunito-x"`;
Proportional = nunito first, then egui defaults.

## Palette (from the mockups)
cream #FFF4E4 (window background), paper #FFFFFF (cards), ink #26203B (text, 2.5 px outlines,
hard shadows), inkSoft #6B6385 (secondary text), coral #FF6B5B (host main action),
coralTint #FFD9D3, mint #3FCB9B (guest main action, done, ok), mintTint #CDF4E5, lilacTint
#ECE6FF (speech bubble, stage), sunTint #FFF0C2 (result card, progress track), sun #FFC94A
(stars, warning), dashed #8A84A0, disabled #B7B1C9, offline-dot #CFCADB.
egui visuals: start from `Visuals::light()` and set panel_fill = cream, window_fill = paper,
text = ink, widgets (combo boxes, text edits, sliders) = paper fill + ink 2 px stroke + radius 12,
selection = lilacTint with ink stroke. Disable animations (`animation_time = 0`), except the
mascot bob.

## Reusable widgets (src/ui/fun.rs)
- `sticker(ui, fill, radius, |ui| ..)`: `egui::Frame` with fill, ink stroke 2.5, corner radius,
  and a **hard** shadow (`Shadow { offset: [0, 5], blur: 0, spread: 0, color: ink }`).
- `fun_button(ui, label, Kind::{Coral, Mint, White, Ghost}) -> Response`: a pill with Fredoka 600
  text, 2.5 px ink stroke and a hard 4 px shadow. On press, draw it 3 px lower with a 1 px shadow
  (the "squish"). Disabled: 45 % opacity. Ghost = inkSoft text, no frame. Hover: slightly lighter fill.
- `pill(ui, text, fill, dot: Option<Color32>)`: status chip with a dot.
- `stone(ui, state: Done|Current|Todo, n, label) -> Response` (clickable): Done = mint circle
  36 px + yellow star (painted polygon) + ink label; Current = coral (host) or mint (guest) 44 px
  with hard shadow + Fredoka label; Todo = white circle with dashed inkSoft outline + inkSoft label.
  The rail paints a dashed vertical connector behind the stones.
- `check_row(ui, ok: bool, text)`: mint tick circle or sun "!" circle + bold text.
- `bubble(ui, mascot, sentence, button: Option<(label, Kind)>) -> Option<clicked>`: mascot
  (84 px) + lilacTint sticker with "NEXT UP" (Nunito 800, 12 px, letter-spaced) and the
  sentence in Fredoka 600 at 20 px.

## Mascots (src/ui/mascot.rs, painted with egui shapes, no images)
Copy geometry from the inline SVGs in the mockups (viewBox 100x100; scale to the target size).
- Host = coral blob + music note (top right); Guest = mint blob + sprout with a sun-coloured bud.
- Bodies are convex: flatten the cubic bezier outline (`CubicBezierShape::flatten`) into a
  closed `PathShape` with fill + ink stroke 3. White cheek ellipses at 45 % alpha, white eyes
  with ink outline, ink pupils + white glints.
- Moods:
  - `Happy`: open singing mouth (dark, pink tongue).
  - `Waiting`: sleepy half-lids in the body colour, a small flat mouth, a "z".
  - `Listening`: pupils looking up, an "o" mouth.
  - `Singing`: closed happy-arc eyes and a big open mouth.
- Optional `bob: f32` vertical offset, `(t * 3).sin() * 3 px`, used only while playing.

## Screens

### First launch (`!cfg.onboarded`): Welcome
Exactly as `docs/ui-mockup/Main.dc.html`:
- the two mascots, "Who's singing today?", and two role cards (coralTint / mintTint);
- buttons "Let's host!" (Coral) and "Join the duet!" (Mint): set `cfg.mode`,
  `onboarded = true`, then `save()`.

"Switch role" in the top bar stops the session (existing `stop`/`stop_session` logic), sets
`onboarded = false` and shows Welcome.

### Main layout (mockups `HostConnect`, `HostMeasured`, `HostSing`, `GuestJoin`)
- **Top bar:** painted note logo, "DuetSync" in Fredoka 700 24 px. Right side: role pill, connection
  pill (`Waiting for partner` sun / `Partner here` mint / `Not connected` grey; for the guest add
  `Clock +-x ms` once synced), OBS pill for the host (`OBS offline` / `OBS connected` /
  `OBS synced` when `cfg.ledger.applied` is non-empty), and the "Switch role" ghost button.
- **Left rail:** a 262 px white sticker titled "Your journey", with the stones. A "Diagnostics"
  ghost button at the bottom opens the Diagnostics view (reuse the current diagnostics panel,
  restyled with stickers).
- **Main column:** bubble (next up), step header ("STEP X OF N", title in Fredoka 700 32 px,
  one-sentence description in inkSoft), the step's content cards, a "+ Advanced settings"
  toggle, and a footer with Ghost "Back" + White "Next: <next step title>".
- **Bottom transport:** a white sticker pill.
  - Host: coral round Play/Pause (painted triangle/bars), White "Stop", a progress track
    (sunTint, ink outline, coral fill) that is draggable to seek, time "m:ss / m:ss", then the
    hint or the sync badge ("In time! +x ms" mint when |err| <= 20 ms, sun otherwise).
  - Guest: "Your host presses play. Just sing along!" + progress + time.
  - Countdown while starting: show "Starting in 3..." in Fredoka 700.

### Steps, completion and "next up" text
`current = selected step` (click a stone to view any step). By default it auto-advances to the
first incomplete step ONLY when a step becomes done while the user is viewing it. A step also
counts as done once it is in `confirmed_steps` (the user pressed Next on it). When the viewed step
is not the next incomplete one, the bubble shows a button `Go to step N` (Coral host / Mint guest).

**Host:**
1. **Find your partner.** Done: `app.connected()`. Content: session card.
   - No session: Coral "Create session" + "Code valid N min" (in Advanced: port).
   - Session: an invite text box (each address) + Coral "Copy invite" (copies the first invite;
     the others get a small White "Copy").
   - Waiting row: faded guest mascot + "Waiting for your partner to join..." + minutes left.
   - White "New code", Ghost "End session".
   - Bubble: no session = "Press Create session to make an invite."; waiting = "Copy the invite
     and send it to your partner on Discord!"; connected = "Your partner is here! Next, set up audio."
2. **Set up audio.** Done: `guest_source_ok` or confirmed. Content:
   - "Your headphones" (output combo + meter);
   - "Your microphone" (combo + "Mic level" toggle + meter);
   - "Your partner's voice" (Discord app / Output device / Input device + Discord found text);
   - Coral "Test partner's voice" (calls `test_guest_capture`).
   - Bubble: "Ask your partner to talk, then press Test partner's voice."
3. **Load the song.** Done: track loaded and (not connected, or `track_matches() == Some(true)`).
   - Content: Coral "Choose song (.wav)" (`pick_wav`), "or drop a .wav on the window", a track card
     (name, length, format), and a match row with `check_row`.
   - Bubble: no track = "Pick the instrumental .wav. Your partner needs the exact same file.";
     mismatch = "Your partner's file is different. Send them the exact same .wav.";
     waiting = "Waiting for your partner to load the same song...".
4. **Measure the delay.** Done: `last_calib` not Invalid (or confirmed with a manual latency).
   - "Before you start" `check_row`s: partner connected; clocks in sync (`remote` present); a sun
     warning "Partner: turn off Krisp and Echo Cancellation in Discord".
   - Coral "Measure delay" (`start_calibration`; disabled with the reason from `can_calibrate`).
   - While running: Listening mascot + progress bar + "Shhh... measuring".
   - Result card (sunTint): big "182 ms" (Fredoka 700 48 px) + pill ("Great measurement" mint /
     "Not sure, measure again" sun / "Didn't work" coralTint), the plain sentence "Your partner's
     voice arrives X ms late. In step 5 we delay your mic and music by the same amount...",
     "N of M test sounds heard", and a collapsible "Show details" (the existing result grid +
     messages).
   - Advanced: count/spacing/level, manual latency, fine tune, use-measured.
   - Show the `staleness()` lines as small inkSoft text.
5. **Send to OBS.** Done: `cfg.ledger.applied` non-empty and every `last_apply` entry verified.
   - Connect card: host/port/password + remember + Coral "Connect OBS", and the help line.
   - "Which OBS source is which?" card: three combos (Your mic / Instrumental / Partner (Discord))
     plus the delays that will be applied, in plain words.
   - Warnings as sun `check_row`s.
   - Coral "Apply to OBS" (opens the existing confirm modal, restyled as a sticker) and White
     "Restore original".
   - Results as `check_row`s.
   - Advanced: safety margin, per-source latency numbers, avatar video delay.
6. **Sing!** Never done. Content: lilacTint "stage" sticker.
   - "NOW SINGING", the track name, and both mascots (Singing mood, bobbing while playing).
   - The sync badge.
   - Three small tiles: Partner status, OBS delay, a tip.
   - A big Coral Play button when stopped (host).
   - Advanced: volume, output latency trim, drift correction, countdown, Resync.

**Guest:**
1. **Join your host.** Done: connected and clock synced. Content: paste field (mint 3 px border)
   + Mint "Join", help text; when connected: "Connected!" + Ghost "Leave".
2. **Set up audio.** Done: `routing_ok` or confirmed.
   - Content: headphones; "Where the test sound plays" (calibration output) + explanation; "Check
     with my microphone" checkbox + input combo + level toggle; Mint "Test sound path"
     (`test_routing`).
   - Bubble explains speakers vs. virtual cable.
3. **Load the song.** Same as the host's step.
4. **Get ready to measure.** Done: `guest_calib_note.is_some()` or confirmed.
   - Discord checklist (Krisp off, echo cancellation off, input sensitivity), test-sound level
     slider, and the note text.
   - Bubble: "Your host will start the measurement. Keep quiet when it starts!"
5. **Sing!** Stage, as the host's, without the Play button.

## Developer preview (for screenshots)
Env var `DUETSYNC_PREVIEW=welcome|host:N|guest:N` makes the GUI start on that view: it sets the
mode, sets `onboarded`, selects step N. It changes ONLY navigation, never any data shown, and is
not saved to the config.

## Final report (max 250 words)
Files changed, how each step's completion is computed, font files and sizes, gate results
(fmt, clippy warnings, test counts), release exe size. Do not commit.
