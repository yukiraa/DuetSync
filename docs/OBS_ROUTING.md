# OBS routing guide (host)

Goal: every sound reaches OBS **exactly once**, from a separate source, so each source can
be delayed independently.

```
Host mic ───────────────► OBS "Mic"              (+ compensation)
DuetSync.exe (music) ───► OBS "Instrumental"     (+ compensation)   Application Audio Capture
Discord.exe (guest) ────► OBS "Discord"          (+ margin only)    Application Audio Capture
Desktop Audio ──────────► muted / removed
```

## Sources

| Role | OBS source type | Select |
| --- | --- | --- |
| Your mic | Audio Input Capture | your microphone |
| Instrumental | Application Audio Capture (Beta) | `[DuetSync.exe]` |
| Guest voice | Application Audio Capture (Beta) | `[Discord.exe]` |

Application Audio Capture needs Windows 10 2004+ and OBS 28+.

### YouTube songs

When the song is a YouTube video there is no `DuetSync.exe` audio to capture. Use the
Browser Source URL shown in step 3 as the **Instrumental** source instead:

1. OBS Sources, +, **Browser**, paste the URL (it ends in `?role=obs`).
2. Size 1920x1080, tick **Control audio via OBS**.
3. In DuetSync step 5 pick that Browser Source as the Instrumental.

The OBS page and your own browser tab are two separate players kept on the same timeline.
Mute your own tab in headphones only if you hear an echo; it is your monitor, not the stream.

## Do not

- **Keep Desktop Audio live.** It contains Discord *and* the instrumental, so the audience
  would hear them twice and out of sync. DuetSync warns about this.
- **Enable "Monitor and Output"** on these sources. It sends the *delayed* audio back to your
  speakers or headphones. If Discord hears it, you get an echo loop. DuetSync warns about
  this too.
- **Route the instrumental into your Discord microphone.** Both performers hear their own
  local copy. Sending it through Discord as well would double it and delay it.

## Guest side

The guest only needs headphones. Their instrumental plays locally, and only their voice goes
through Discord. If the guest uses a virtual cable for calibration, they switch Discord's
input back to the real microphone afterwards. Or they keep the cable and mix the mic into it,
whichever they prefer.

## Video (optional, manual)

The guest's camera or VTube Studio feed (e.g. Discord screen share captured in OBS) has its
own latency, which differs from the audio. In **OBS compensation → Avatar video delay**,
pick the source and a delay (0–500 ms; OBS's Render Delay limit). DuetSync adds or updates a
filter named `DuetSync Delay`. **Remove** deletes it. Match lips by eye. This is manual,
because DuetSync has no video reference signal.

## How the offsets are computed

```
target = max(latencies) + margin
offset = baseline + (target − latency)
```

`baseline` is the offset the input had before DuetSync first changed it. It is stored in the
config so that "Restore original offsets" works across restarts. If you edit an offset in OBS
yourself, DuetSync notices: the current value no longer matches what it wrote. It then uses
your value as the new baseline and tells you.
