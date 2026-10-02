//! Optional graphical presentation in the current terminal.
//!
//! Application controllers own all commands and IO. This module owns the
//! transport, rendering process, terminal presentation and reusable scene.
pub mod assets;
pub mod capabilities;
pub mod cells;
pub mod client;
mod native;
pub mod protocol;
pub mod renderer;
#[cfg(unix)]
pub mod session;

pub use protocol::{Component, Input, Rect, Scene, Viewport};
