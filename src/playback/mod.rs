//! Synchronized instrumental playback: track identity, streaming feeder, player core.

pub mod core;
pub mod track;

pub use track::{Feeder, TrackInfo};
