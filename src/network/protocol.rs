//! Session messages. JSON inside length-prefixed (and after handshake, encrypted) frames.

use crate::calibration::CalibrationPlan;
use crate::playback::TrackInfo;
use serde::{Deserialize, Serialize};

/// v2: `Track`/`TrackStatus` became `Media`/`MediaStatus` (WAV or YouTube) and the guest
/// streams `VoiceEnvelope`s for live auto-sync. v1 peers are refused at the handshake.
pub const PROTO_VERSION: u32 = 2;
pub const MAX_FRAME: usize = 64 * 1024;
pub const DEFAULT_PORT: u16 = 47_800;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Handshake {
    Hello {
        proto: u32,
        app: String,
        nonce: String,
    },
    Challenge {
        proto: u32,
        app: String,
        nonce: String,
    },
    Auth {
        mac: String,
    },
    AuthOk {
        mac: String,
    },
    Reject {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GuestCalibReport {
    /// Host-clock time each chirp was detected in the guest's own reference capture
    /// (`None` = not detected or no reference capture configured).
    pub mic_times_host_us: Vec<Option<i64>>,
    pub reference_capture: bool,
    /// Guest clock-offset uncertainty at the time of the run.
    pub clock_uncertainty_us: i64,
    pub errors: Vec<String>,
}

/// What the host is singing along to.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum MediaInfo {
    /// An instrumental .wav; both sides compare the SHA-256.
    Wav(TrackInfo),
    /// A YouTube video; both sides compare the video id.
    YouTube { video_id: String },
}

/// Largest `VoiceEnvelope` payload (1 ms bins) so one frame stays well under `MAX_FRAME`.
pub const MAX_ENVELOPE_BINS: usize = 8_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Msg {
    Ping {
        seq: u32,
        t1: i64,
    },
    Pong {
        seq: u32,
        t1: i64,
        t2: i64,
        t3: i64,
    },
    /// Host announces (or clears) the song to verify.
    Media(Option<MediaInfo>),
    /// Guest's answer: `kind` is "wav" (`id_or_sha` = SHA-256) or "youtube" (`id_or_sha` =
    /// video id); `name` is the file name for WAV.
    MediaStatus {
        kind: String,
        id_or_sha: Option<String>,
        name: Option<String>,
    },
    /// Guest to host: 1 ms voice-energy bins (`live::to_u8`) starting at `start_host_ms` on
    /// the host clock. About 1 KB per second of singing.
    VoiceEnvelope {
        start_host_ms: i64,
        bins: Vec<u8>,
    },
    Play {
        gen: u32,
        start_host_us: i64,
        from_frame: u64,
    },
    Pause {
        gen: u32,
        at_frame: u64,
    },
    Stop,
    CalibStart(CalibrationPlan),
    CalibReport(GuestCalibReport),
    GuestStatus {
        state: String,
        pos_frames: f64,
        at_host_us: i64,
        clock_uncertainty_us: i64,
        rtt_us: i64,
        drift_ms: f64,
    },
    Notice(String),
    Bye,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_and_envelope_roundtrip_under_frame_limit() {
        let m = Msg::Media(Some(MediaInfo::YouTube {
            video_id: "dQw4w9WgXcQ".into(),
        }));
        let back: Msg = serde_json::from_slice(&serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        let env = Msg::VoiceEnvelope {
            start_host_ms: 1_234_567,
            bins: vec![255; MAX_ENVELOPE_BINS],
        };
        let bytes = serde_json::to_vec(&env).unwrap();
        assert!(bytes.len() + 64 < MAX_FRAME, "{} bytes", bytes.len());
        let back: Msg = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, env);
    }
}
