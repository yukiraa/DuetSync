# Third-party notices

DuetSync is MIT licensed. It is built from the following open-source Rust crates, which are
statically linked into `DuetSync.exe`. Licenses are as declared in each crate's metadata
(`cargo metadata`).

## Direct dependencies

| Crate | License | Purpose |
| --- | --- | --- |
| eframe / egui / epaint / emath / ecolor | MIT OR Apache-2.0 | native GUI (OpenGL via glow) |
| epaint_default_fonts | (MIT OR Apache-2.0) AND OFL-1.1 AND Ubuntu-font-1.0 | bundled fonts (Ubuntu, Hack, Noto Emoji, emoji-icon-font) |
| windows / windows-core | MIT OR Apache-2.0 | Win32 / WASAPI bindings |
| rustfft | MIT OR Apache-2.0 | FFT for cross-correlation |
| tungstenite | MIT OR Apache-2.0 | OBS WebSocket client |
| serde / serde_json | MIT OR Apache-2.0 | config and protocol |
| sha2 / hmac | MIT OR Apache-2.0 | file identity, authentication |
| chacha20poly1305 | Apache-2.0 OR MIT | encrypted session transport |
| base64 | MIT OR Apache-2.0 | OBS authentication |
| getrandom | MIT OR Apache-2.0 | join-code secrets |
| hound | Apache-2.0 | WAV decoding |

## Transitive dependencies

About 280 further crates (winit, glutin, glow, raw-window-handle, etc.). They use
permissive licenses: MIT, Apache-2.0, BSD-2/3-Clause, Zlib, ISC, Unlicense, 0BSD,
Unicode-3.0, or dual or triple combinations of these. Where a crate offers a choice
(e.g. "MIT OR Apache-2.0 OR LGPL-2.1-or-later"), DuetSync uses it under MIT or Apache-2.0.
Regenerate the full list with:

```
cargo metadata --format-version 1 | jq -r '.packages[] | "\(.name) \(.version) \(.license)"'
```

The full license texts are in each crate's source distribution on crates.io.

## Fonts

- Ubuntu font: Ubuntu Font Licence 1.0
- Hack: MIT / Bitstream Vera license
- Noto Emoji: SIL Open Font License 1.1
- emoji-icon-font: MIT / OFL
- Fredoka (SemiBold, Bold) and Nunito (Regular, ExtraBold): SIL Open Font License 1.1,
  bundled in `assets/fonts/` and embedded in `DuetSync.exe`. License texts:
  `assets/fonts/OFL-Fredoka.txt` and `assets/fonts/OFL-Nunito.txt`.

OBS Studio, obs-websocket, Discord, Tailscale and VB-CABLE are separate products. They are
not distributed with DuetSync.
