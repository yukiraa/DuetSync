//! DuetSync core library: audio, calibration DSP, clock sync, networking, OBS, playback.
//! The GUI lives in `ui` + `app`; everything else is UI-independent and unit-tested.

pub mod app;
pub mod audio;
pub mod calibration;
pub mod clock;
pub mod config;
pub mod diagnostics;
pub mod network;
pub mod obs;
pub mod playback;
pub mod ring;
pub mod selftest;
pub mod ui;

pub const APP_NAME: &str = "DuetSync";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Internal sample rate used for every stream we open (WASAPI converts if needed).
pub const SAMPLE_RATE: u32 = 48_000;
