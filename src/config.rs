//! Persistent JSON configuration. Portable mode: if `DuetSync.json` exists next to the
//! executable it is used; otherwise `%APPDATA%\DuetSync\config.json`.

use crate::obs::compensation::OffsetLedger;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Host,
    Guest,
}

/// Host-side capture of the guest's voice.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum GuestAudio {
    /// Process loopback of the running Discord client (auto-detected).
    #[default]
    DiscordProcess,
    /// Loopback of an output device that only Discord plays to.
    OutputLoopback(Option<String>),
    /// An input device (e.g. a virtual cable carrying Discord's output).
    Input(Option<String>),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VideoCfg {
    pub host_source: Option<String>,
    pub guest_source: Option<String>,
    pub host_ms: i64,
    pub guest_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// False until the user has picked a role on the welcome screen.
    pub onboarded: bool,
    // Session
    pub listen_port: u16,
    pub host_address: String,
    pub join_code_minutes: u64,
    // Audio devices (None = system default)
    pub output_device: Option<String>,
    pub mic_device: Option<String>,
    /// Guest: where calibration chirps are played (speaker near mic, or a virtual cable
    /// that is Discord's input).
    pub calib_output_device: Option<String>,
    /// Guest: optional capture used to timestamp chirps entering the mic.
    pub guest_reference_input: Option<String>,
    pub use_guest_reference: bool,
    /// Host: where the guest's voice (Discord output) is captured for calibration.
    pub guest_audio: GuestAudio,
    // Calibration
    pub chirp_count: usize,
    pub chirp_spacing_ms: f64,
    pub chirp_amplitude: f32,
    pub manual_guest_latency_ms: f64,
    pub fine_tune_ms: f64,
    pub host_mic_latency_ms: f64,
    pub music_latency_ms: f64,
    pub margin_ms: f64,
    // OBS
    pub obs_host: String,
    pub obs_port: u16,
    pub remember_obs_password: bool,
    pub obs_password: Option<String>,
    pub obs_host_mic: Option<String>,
    pub obs_guest: Option<String>,
    pub obs_music: Option<String>,
    pub ledger: OffsetLedger,
    pub video: VideoCfg,
    // Playback
    pub last_track: Option<PathBuf>,
    pub output_latency_ms: f64,
    pub volume: f32,
    pub drift_correction: bool,
    pub countdown_s: f64,
    /// Measure the Discord delay from the singing itself and re-apply it to OBS.
    pub auto_sync: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Host,
            onboarded: false,
            listen_port: crate::network::DEFAULT_PORT,
            host_address: String::new(),
            join_code_minutes: 30,
            output_device: None,
            mic_device: None,
            calib_output_device: None,
            guest_reference_input: None,
            use_guest_reference: true,
            guest_audio: GuestAudio::DiscordProcess,
            chirp_count: 8,
            chirp_spacing_ms: 1500.0,
            chirp_amplitude: 0.5,
            manual_guest_latency_ms: 0.0,
            fine_tune_ms: 0.0,
            host_mic_latency_ms: 0.0,
            music_latency_ms: 0.0,
            margin_ms: 20.0,
            obs_host: "127.0.0.1".into(),
            obs_port: 4455,
            remember_obs_password: false,
            obs_password: None,
            obs_host_mic: None,
            obs_guest: None,
            obs_music: None,
            ledger: OffsetLedger::default(),
            video: VideoCfg::default(),
            last_track: None,
            output_latency_ms: 0.0,
            volume: 1.0,
            drift_correction: true,
            countdown_s: 4.0,
            auto_sync: true,
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.with_file_name("DuetSync.json");
        if p.exists() {
            return p;
        }
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("DuetSync").join("config.json")
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        let p = config_path();
        match std::fs::read(&p) {
            Ok(b) => match serde_json::from_slice::<Config>(&b) {
                Ok(c) => (c.sanitized(), None),
                Err(e) => (
                    Self::default(),
                    Some(format!(
                        "config {} unreadable ({e}); using defaults",
                        p.display()
                    )),
                ),
            },
            Err(_) => (Self::default(), None),
        }
    }

    fn sanitized(mut self) -> Self {
        self.chirp_count = self.chirp_count.clamp(4, 16);
        self.chirp_spacing_ms = self.chirp_spacing_ms.clamp(800.0, 3000.0);
        self.chirp_amplitude = self.chirp_amplitude.clamp(0.05, 0.9);
        self.margin_ms = self.margin_ms.clamp(0.0, 500.0);
        self.volume = self.volume.clamp(0.0, 2.0);
        self.countdown_s = self.countdown_s.clamp(2.0, 15.0);
        self.join_code_minutes = self.join_code_minutes.clamp(1, 24 * 60);
        if !self.remember_obs_password {
            self.obs_password = None;
        }
        self
    }

    pub fn save(&self) -> Result<(), String> {
        let p = config_path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let mut c = self.clone();
        if !c.remember_obs_password {
            c.obs_password = None;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&c).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_password_not_persisted_by_default() {
        let mut c = Config {
            obs_password: Some("secret".into()),
            ..Default::default()
        };
        c.ledger.baseline.insert("Mic".into(), 12);
        let s = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&s).unwrap();
        assert_eq!(back.ledger.baseline["Mic"], 12);
        assert_eq!(back.sanitized().obs_password, None);
        // Unknown/missing fields tolerated.
        let partial: Config = serde_json::from_str(r#"{"margin_ms": 30.0, "future": 1}"#).unwrap();
        assert_eq!(partial.margin_ms, 30.0);
        assert_eq!(partial.obs_port, 4455);
    }
}
