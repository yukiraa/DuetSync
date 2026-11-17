# Troubleshooting

## Connection

| Message / symptom | Cause and fix |
| --- | --- |
| `cannot reach host …` | Only shown for the same-Wi-Fi path; the internet path is tried at the same time. Read the join screen's status line. If it says the networks are too strict, try a phone hotspot on one side. |
| `invalid join code` | Code mistyped or regenerated. Paste the full invite `ip:port#CODE`. |
| `join code expired` | New joins are refused after the validity time. Host: **New code**. |
| `protocol version mismatch` | Both people need the same DuetSync version. |
| `peer timed out (no heartbeat for 5 s)` | Network dropped. The guest reconnects automatically (retries every 2 s at most). |
| Clock uncertainty > 10 ms | Busy or asymmetric network. Wait 20–30 s; it improves as the fastest exchanges accumulate. Wi-Fi with power saving is a common cause. |

## Calibration

| Symptom | Fix |
| --- | --- |
| `0 of 8` / `signal not found` | Guest: Discord Noise Suppression (Krisp) and Echo Cancellation **off**; Voice Activity sensitivity low enough; calibration output reaches the mic (**Test routing**). Host: correct **Guest voice source** (Test guest source while the guest talks). |
| `not heard by guest's reference input` | The guest's *Verify with my input* device didn't hear the sweeps. Fix routing, or untick it. Then the scheduled times are used, and the guest's speaker→mic delay is included. |
| `ambiguous correlation` | Strong echo or reverb. Lower the speaker volume, move the mic, or use a virtual cable. |
| `clipped` | Lower the *Test signal level* or the speaker volume. |
| Low confidence / high jitter | Unstable Discord path. Recalibrate; consider a larger safety margin. |
| Process loopback fails | Windows older than 10 2004, or Discord not running. Use *Output device* (a device only Discord plays to) or *Input device* (a virtual cable). |

## Playback

| Symptom | Fix |
| --- | --- |
| `instrumental does not match` | Files differ byte-for-byte. Share the exact WAV; don't re-export it. |
| `clock not synchronized yet` | Guest joined less than ~2 s ago. Wait for `clock ±x ms`. |
| Music late on Bluetooth headphones | Set **Output latency trim** to your headset's extra delay (often 150–250 ms). Wired headphones are strongly recommended. |
| `guest timeline` keeps growing | Enable *Gradual drift correction*, or press **Resync now**. |
| Underruns increase | PC overloaded or disk very slow. Close heavy apps; copy the WAV to a local SSD. |

## OBS

| Message | Fix |
| --- | --- |
| `cannot reach OBS` | OBS not running, or WebSocket server disabled (Tools → WebSocket Server Settings). |
| `OBS rejected the WebSocket password` | Re-enter the password from OBS's WebSocket settings. |
| `refusing to connect to OBS on a public address` | By design. Run DuetSync on the same PC as OBS, or on a private network. |
| `outside OBS range` | OBS offsets must be between −950 and 20000 ms. Your measured latency or margin is unrealistic. |
| Offsets FAILED in the verify list | OBS reported a different value. Refresh and try again. |

## Getting help

**Diagnostics → Export diagnostics** writes `duetsync-diagnostics-<time>.txt` next to the
config. Passwords and join codes are redacted. Run `DuetSync.exe --selftest` from a terminal
to check audio devices.
