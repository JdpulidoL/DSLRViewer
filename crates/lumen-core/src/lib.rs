//! Everything Lumen does that is not drawing: PTP codec, vendor drivers, the camera engine thread,
//! frame analysis and focus stacking. Shared by the egui desktop app and the Android bindings.

// Byte-chunk helpers in the codecs read clearer with `chunks_exact` than with const-generic `as_chunks`.
#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod camera;
pub mod config;
pub mod engine;
pub mod imaging;
pub mod link;
pub mod logfile;
pub mod ptp;

#[cfg(test)]
mod e2e;
