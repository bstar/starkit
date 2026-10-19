//! Original-file byte ranges and adaptive HLS for native players inside SSH.
//!
//! HLS uses a supervised external FFmpeg; no codec, audio-device or UI linkage.
//! Catalog documents belong to the application; all media URLs use opaque IDs.
mod hls;
mod server;
pub mod ssh;
pub use server::{Resource, Server, Session};

#[cfg(unix)]
mod control;
