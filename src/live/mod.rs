//! Live Discord-delay tracking from real singing.
//!
//! The guest sends a compact, clock-synchronised envelope of its microphone.
//! The host correlates that with the envelope from Discord capture and can
//! refresh the OBS compensation when the measured path drifts.  It needs
//! singing (or other matching voice) on both sides of the path; Krisp and
//! similar voice processing can distort the envelope enough to reject it.

pub mod envelope;
pub mod tracker;
pub mod worker;

pub use envelope::{from_u8, to_u8, EnvelopeBuilder};
pub use tracker::{LiveEstimate, LiveFilter, LiveTracker};
pub use worker::{LiveBins, LiveCapture};
