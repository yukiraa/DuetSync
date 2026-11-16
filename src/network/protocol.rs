//! Session messages. JSON inside length-prefixed (and after handshake, encrypted) frames.

use crate::calibration::CalibrationPlan;
use crate::playback::TrackInfo;
use serde::{Deserialize, Serialize};

pub const PROTO_VERSION: u32 = 1;
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
    /// Host announces (or clears) the instrumental to verify.
    Track(Option<TrackInfo>),
    TrackStatus {
        sha256: Option<String>,
        name: Option<String>,
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
