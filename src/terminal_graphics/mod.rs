//! Optional graphical presentation in the current terminal.
//!
//! Application controllers own all commands and IO. This module owns the
//! transport, rendering process, terminal presentation and reusable scene.
pub mod assets;
pub mod capabilities;
pub mod cells;
pub mod client;
pub mod protocol;
pub mod renderer;
#[cfg(unix)]
pub mod session;

pub use protocol::{Component, Input, Rect, Scene, Viewport};

/// Packaged, trusted runtime. Remote peers supply data, never HTML or scripts.
pub const RUNTIME_MAIN: &str = include_str!("../../runtime/terminal-graphics/main.cjs");
pub const RUNTIME_HTML: &str = include_str!("../../runtime/terminal-graphics/index.html");
pub const RUNTIME_PRELOAD: &str = include_str!("../../runtime/terminal-graphics/preload.cjs");
