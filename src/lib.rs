//! The terminal-UI foundation shared by staramp and starcord.
//!
//! Not a general TUI framework. Every item here came out of one of the two
//! applications that use it, and the shape of the API is whatever those two
//! needed rather than whatever a third might. That is a deliberate limit: a
//! library with two known consumers can be changed by reading both of them.
//!
//! ## One copy of ratatui
//!
//! `ratatui`, `crossterm` and -- with the `image` feature -- `ratatui_image`
//! and `image` are re-exported from here, and both applications reach them
//! through this crate rather than depending on them directly. A `Widget`
//! implemented against a second copy of ratatui does not satisfy a signature
//! expecting the first, and the compiler reports that as a mismatch between
//! two versions carrying the same number, which sends the reader looking for a
//! problem that is not there.

pub mod config;
pub mod digits;
pub mod fs;
#[cfg(feature = "image")]
pub mod graphics;
pub mod list;
pub mod logging;
pub mod mouse;
pub mod paths;
pub mod term;
pub mod text;
pub mod theme;

pub use crossterm;
pub use ratatui;

#[cfg(feature = "image")]
pub use image;
#[cfg(feature = "image")]
pub use ratatui_image;
