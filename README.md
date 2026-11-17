# DuetSync

**Sing together, anywhere.**

![DuetSync main window](docs/screenshot.png)

![Guided steps](docs/screenshot-steps.png)

DuetSync is a small, free, open-source (MIT) Windows app for two performers who sing a
karaoke duet over Discord and want the **recording or livestream** to sound in time.

It does **not** remove internet latency. You will still hear your partner late in Discord,
as always. What DuetSync does:

1. Both of you hear the **same instrumental at the same moment**. Each PC plays its own
   local copy, and playback starts on a shared clock.
2. It **measures** how late your partner's voice reaches the host PC through Discord.
3. It **delays the host's own mic and instrumental in OBS** by that amount, so the audience
   hears both voices and the music lined up.

No account, no subscription, no cloud server, no AI, no browser engine, no Discord
modification, no audio driver to install.

---

## Requirements

- Windows 10 2004 or newer (Windows 11 recommended), 64-bit.
- Both performers: Discord (normal app), headphones, and the **identical** instrumental as a
  `.wav` file. Convert MP3s to WAV with any audio tool; both people must use the very same
  WAV file.
- Host only: OBS Studio 28+ (it includes obs-websocket 5).
- A network path between the two PCs: the same LAN, or a private VPN such as
  [Tailscale](https://tailscale.com) (free for personal use). Do **not** port-forward DuetSync
  to the public internet.

## Install

Download `DuetSync.exe` and run it. It is a single portable file.

- Settings are saved to `%APPDATA%\DuetSync\config.json`.
- **Portable mode:** put a `DuetSync.json` file next to the exe and settings are stored
  there instead. You can copy `DuetSync.example.json` to start.

Windows SmartScreen may warn about an unsigned app. Choose *More info → Run anyway* only if
you trust where you got the file (or build it yourself, see below).

---

## How the app guides you

On first launch DuetSync asks **"Who's singing today?"**. Pick **Let's host!** (you run OBS) or
**Join the duet!** (your partner invited you). You can change it later with **Switch role**.

After that, the left side shows **Your journey**: numbered steps. A star means a step is done,
and the big coloured circle is where you are. The mascot at the top always says what to do next
(**NEXT UP**). Each screen has one coloured button: coral for the host, mint for the guest. That
is the one to press. **Next** moves on; you can click any step to go back. Rarely needed options
are under **+ Advanced settings**.

### Before you start (host, once)

In OBS: *Tools → WebSocket Server Settings → Enable WebSocket server*, and set a password.
Create these audio sources (see [docs/OBS_ROUTING.md](docs/OBS_ROUTING.md)):
- your **microphone**;
- **Application Audio Capture** of **Discord** (your partner's voice);
- **Application Audio Capture** of **DuetSync** (your instrumental).

Mute or remove *Desktop Audio*, because it would add Discord and the music a second time.

### Host journey

1. **Find your partner:** press **Create session**, then **Copy invite**. Send it to your partner
   in a **private** Discord message. New joins stop when the code expires; a partner who
   already joined can always reconnect.
2. **Set up audio:** choose your headphones and microphone. "Your partner's voice" is
   normally **Discord app**. Press **Test partner's voice** while your partner talks.
3. **Load the song:** press **Choose song (.wav)** or drop the file on the window. Both of you
   need the exact same file (checked by fingerprint).
4. **Measure the delay:** press **Measure delay** and stay quiet for about 15 s (details below).
5. **Send to OBS:** connect OBS, tell DuetSync which source is which, press **Apply to OBS**.
6. **Sing!:** press **Play**. You both hear the music at the same moment.

### Guest journey

1. **Join your host:** paste the invite and press **Join**.
2. **Set up audio:** choose your headphones. "Where the test sound plays" must be something
   your **Discord microphone can hear**: speakers near the mic, or a virtual cable (e.g.
   VB-CABLE) set as Discord's input. Headphones do **not** reach the mic. Press **Test sound
   path**; it must say the sound was heard.
3. **Load the song:** the same .wav file as your host.
4. **Get ready to measure:** in **Discord → Voice & Video**, turn **off** Noise Suppression
   (Krisp) and Echo Cancellation, and let quiet sounds through. Your host starts the measurement.
5. **Sing!:** your host presses Play.

---

## Calibrate

Host: step 4 **Measure the delay** → **Measure delay**. Stay quiet for ~15 seconds.

- The guest's PC plays 8 short sweeps at moments fixed on the shared clock.
- They travel through the guest's mic → Discord → the host's PC.
- DuetSync finds each one by cross-correlation and reports the **median latency**, the
  **jitter**, an **uncertainty** range, and a **confidence**.
- Weak, missing, ambiguous or clipped sweeps are rejected. If fewer than 4 survive, the
  measurement is **rejected**: no number is invented.

Under **+ Advanced settings** you can instead type a **manual latency**, and use **Fine tune** (±ms) after listening to
a test recording.

Discord's delay changes over time and DuetSync cannot see it during a song. **Recalibrate
between songs.** The app warns when the measurement is old or when the network round trip
has changed.

## Apply to OBS

Host: step 5 **Send to OBS**:

```
target          = largest source latency + safety margin
offset (source) = your original offset + (target − source latency)
```

Example: guest latency 180 ms, margin 20 ms → your mic +200 ms, instrumental +200 ms,
Discord +20 ms.

Press **Apply to OBS**, review the table (original / now / new), and confirm.
DuetSync reads every value back from OBS and marks each one "verified" or "FAILED". Your original offsets are
remembered, so recalibrating **replaces** the compensation and never adds to it.
**Restore original** puts them back.

*Avatar video delay (manual, under + Advanced settings):* adds an OBS **Render Delay** filter (0–500 ms) to a host or
guest video source. Video latency is **not** measured. Set it by eye.

## Sing

Host presses **Play** (step 6 or the play bar at the bottom). Both PCs count down and start the instrumental at the same moment.
The host sees an **In time!** badge with how far the guest's player is from the host's.
Pause, Stop and seeking are mirrored to the guest. **Resync** restarts both players in
two seconds if something drifted.

Monitor yourselves with headphones the normal way (Discord + local music). **Never**
monitor through OBS's delayed output.

---

## Troubleshooting

See [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md). Quick checks:

| Symptom | Fix |
| --- | --- |
| Guest can't connect | Same LAN/Tailscale? Firewall prompt allowed on host? Invite pasted completely? |
| "join code expired" | Host presses **New code** and sends the new invite. |
| Calibration rejected, 0 signals | Guest's Krisp/echo cancellation on, wrong calibration output, or test routing failed. |
| "Partner's file differs" | Not the same WAV (re-exported files differ). Send the exact file. |
| Echo in the recording | Desktop Audio or monitoring enabled in OBS. See routing guide. |

**Diagnostics** (bottom of the journey) → **Export diagnostics** writes a text report (passwords and join codes
redacted) next to the config file.

## Command line

```
DuetSync.exe --selftest              # audio hardware check (plays sweeps on default output)
DuetSync.exe --selftest --bench 30   # + idle CPU/RAM with an output stream open
DuetSync.exe --selftest --e2e        # host+guest on this PC through a VB-CABLE (silent)
DuetSync.exe --print-default-config  # example configuration
```

## Build from source

```
rustup default stable
cargo test
cargo build --release      # -> target\release\DuetSync.exe
```

## Documentation

- [OBS routing guide](docs/OBS_ROUTING.md)
- [Troubleshooting](docs/TROUBLESHOOTING.md)
- [Known limitations](docs/LIMITATIONS.md)
- [Test report](docs/TEST_REPORT.md) and the real two-computer checklist
- [Performance report](docs/PERFORMANCE.md)
- [Architecture](docs/ARCHITECTURE.md)

## License

MIT. See [LICENSE](LICENSE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
