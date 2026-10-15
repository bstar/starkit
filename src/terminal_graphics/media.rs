//! Negotiated wire messages, independent of installed media engines.
use crate::media_types as media;
use crate::media_types::Quality;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToClient {
    CanvasVideoOpen {
        session: u64,
        generation: u64,
        stream: String,
        width: u32,
        height: u32,
    },
    CanvasVideoPacket {
        session: u64,
        generation: u64,
        data: String,
    },
    CanvasVideoClose {
        session: u64,
        generation: u64,
        stream: String,
    },
    /// Enable full-song caching for a negotiated audio_cache_v1 frontend.
    PcmPause {
        session: u64,
        epoch: u64,
        paused: bool,
    },
    PcmCache {
        session: u64,
        epoch: u64,
    },
    PcmOpen {
        session: u64,
        epoch: u64,
        spec: crate::pcm::Spec,
        compatibility: bool,
    },
    PcmChunk {
        session: u64,
        epoch: u64,
        format: crate::pcm::SampleFormat,
        processed: bool,
        data: String,
    },
    PcmClose {
        session: u64,
        epoch: u64,
    },
    PcmEnd {
        session: u64,
        epoch: u64,
    },
    OriginalOpen {
        session: u64,
        generation: u64,
        image: String,
        size: u64,
        name: String,
        start: f64,
        options: media::PlaybackOptions,
    },
    Range {
        session: u64,
        generation: u64,
        offset: u64,
        data: String,
    },
    AudioOpen {
        session: u64,
        epoch: u64,
    },
    AudioChunk {
        session: u64,
        epoch: u64,
        samples: Vec<i16>,
    },
    AudioClose {
        session: u64,
    },
    Open {
        session: u64,
        generation: u64,
        image: String,
        local: Option<PathBuf>,
        start: f64,
        quality: Quality,
        #[serde(default)]
        options: media::PlaybackOptions,
    },
    Chunk {
        session: u64,
        generation: u64,
        data: String,
    },
    Eof {
        session: u64,
        generation: u64,
    },
    Control {
        session: u64,
        generation: u64,
        paused: bool,
        volume: u8,
    },
    Close {
        session: u64,
        generation: u64,
    },
    Error {
        session: u64,
        generation: u64,
        message: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToHost {
    CanvasVideoCredit {
        session: u64,
        generation: u64,
    },
    CanvasVideoError {
        session: u64,
        generation: u64,
        message: String,
    },
    /// Source frames received and stored on the presentation machine.
    PcmBuffered {
        session: u64,
        epoch: u64,
        frames: u64,
    },
    PcmProgress {
        session: u64,
        epoch: u64,
        frames: u64,
    },
    PcmFinished {
        session: u64,
        epoch: u64,
    },
    PcmReady {
        session: u64,
        epoch: u64,
        sample_rate: u32,
        channels: u16,
        format: String,
        converted: bool,
    },
    Read {
        session: u64,
        generation: u64,
        offset: u64,
    },
    AudioCredit {
        session: u64,
        epoch: u64,
        blocks: usize,
    },
    AudioError {
        session: u64,
        epoch: u64,
        message: String,
    },
    Credit {
        session: u64,
        generation: u64,
        bytes: usize,
    },
    Status {
        session: u64,
        generation: u64,
        position: f64,
        buffering: bool,
        finished: bool,
        warning: Option<String>,
        #[serde(default)]
        bandwidth_bps: u64,
        #[serde(default)]
        buffered_bytes: u64,
        #[serde(default)]
        dropped_frames: u64,
        #[serde(default)]
        source_bitrate: u64,
    },
}

#[cfg(feature = "media")]
#[path = "media_runtime.rs"]
mod runtime;
#[cfg(feature = "media")]
pub use runtime::{Frontend, Host};
#[cfg(not(feature = "media"))]
#[path = "media_unavailable.rs"]
mod unavailable;
#[cfg(not(feature = "media"))]
pub use unavailable::Frontend;
