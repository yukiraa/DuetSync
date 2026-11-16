# DuetSync

**Sing together, anywhere.**

![DuetSync main window](docs/screenshot.png)

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

## First-time setup (5 minutes)

### Host (the person running OBS)

1. **OBS:** *Tools → WebSocket Server Settings → Enable WebSocket server*. Note the port
   (default 4455) and set a password.
2. In OBS, create these audio sources (see [docs/OBS_ROUTING.md](docs/OBS_ROUTING.md)):
   - your **microphone**;
   - **Application Audio Capture** of **Discord** (your partner's voice);
   - **Application Audio Capture** of **DuetSync** (your instrumental).
   Mute or remove *Desktop Audio*, because it would add Discord and the music a second time.
3. Open DuetSync and choose **Host** at the top right.
4. **Audio** panel:
   - *Instrumental output*: your headphones.
   - *Guest voice source*: **Discord app** (auto-detected). Press **Test guest source**
     while your partner talks. It should report "audio present".
5. **Session → Create session.** Copy the invite (looks like `100.64.1.2:47800#ABCDE-…`)
   and send it to your partner in a **private** message. The code expires for new joins after
   the time you set (reconnects keep working).
6. **OBS compensation** tab → enter the OBS password → **Connect OBS**.
   Assign *Your mic*, *Instrumental* and *Guest (Discord)* to the matching OBS inputs.

### Guest

1. Open DuetSync and choose **Guest**.
2. Paste the invite into **Session** and press **Join session**. When the clock shows
   `clock ±x ms` at the top right, you are synchronized.
3. **Audio** panel:
   - *Instrumental output*: your headphones.
   - *Calibration signal output*: something your **Discord microphone can hear**:
     - **speakers** placed near your mic, **or**
     - a **virtual audio cable** (e.g. VB-CABLE) that you selected as Discord's input
       device for the calibration.
     Playing the test signal into headphones will **not** reach your microphone.
   - *Verify with my input*: select your microphone (or the cable's output). Press
     **Test routing**. It must say "routing OK".
4. In **Discord → Voice & Video**, for the calibration only: turn **off** Noise Suppression
   (Krisp), **off** Echo Cancellation, and make sure input sensitivity lets the sweeps
   through. You can turn them back on afterwards.

### Both

**Player** tab → **Load WAV…** (or drag the file onto the window). The Player shows
"Partner has the identical file" when the SHA-256 fingerprints match. Playback is refused
if they differ.

---

## Calibrate

Host: **Calibrate** tab → **Calibrate**. Stay quiet for ~15 seconds.

- The guest's PC plays 8 short sweeps at moments fixed on the shared clock.
- They travel through the guest's mic → Discord → the host's PC.
- DuetSync finds each one by cross-correlation and reports the **median latency**, the
  **jitter**, an **uncertainty** range, and a **confidence**.
- Weak, missing, ambiguous or clipped sweeps are rejected. If fewer than 4 survive, the
  measurement is **rejected**: no number is invented.

You can instead type a **manual latency**, and use **Fine tune** (±ms) after listening to
a test recording.

Discord's delay changes over time and DuetSync cannot see it during a song. **Recalibrate
between songs.** The app warns when the measurement is old or when the network round trip
has changed.

## Apply to OBS

**OBS compensation** tab:

```
target          = largest source latency + safety margin
offset (source) = your original offset + (target − source latency)
```

Example: guest latency 180 ms, margin 20 ms → your mic +200 ms, instrumental +200 ms,
Discord +20 ms.

Press **Apply compensation…**, review the table (original / now / new), and confirm.
DuetSync reads every value back from OBS and marks each one "verified" or "FAILED". Your original offsets are
remembered, so recalibrating **replaces** the compensation and never adds to it.
**Restore original offsets** puts them back.

*Avatar video delay (manual):* adds an OBS **Render Delay** filter (0–500 ms) to a host or
guest video source. Video latency is **not** measured. Set it by eye.

## Sing

Host presses **Play**. Both PCs count down and start the instrumental at the same moment.
The host sees `guest timeline ±x ms` (how far the guest's player is from the host's).
Pause, Stop and seeking are mirrored to the guest. **Resync now** restarts both players in
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

**Diagnostics** tab → **Export diagnostics** writes a text report (passwords and join codes
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
