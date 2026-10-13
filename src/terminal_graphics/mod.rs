//! Optional graphical presentation in the current terminal.
//!
//! Application controllers own all commands and IO. This module owns the
//! transport, rendering process, terminal presentation and reusable scene.
pub mod assets;
pub mod capabilities;
mod cell_video;
pub mod cells;
pub mod client;
mod drop_bridge;
mod font;
mod mouse;
mod native;
pub mod placement;
mod pointer;
pub mod protocol;
pub mod renderer;
pub use crate::native_surface as surface;
#[cfg(unix)]
pub mod session;
#[cfg(unix)]
pub mod terminal_bridge;

pub use protocol::{Component, Input, Rect, Scene, Viewport};

pub mod media;
#[cfg(feature = "media")]
mod original;

#[cfg(feature = "audio-output")]
pub mod audio;

pub mod window;

pub mod video_presenter;

#[cfg(feature = "audio-output")]
mod audio_v2;
#[cfg(feature = "audio-output")]
mod pcm_cache;
