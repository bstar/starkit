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

pub use crossterm;
pub use ratatui;

#[cfg(feature = "image")]
pub use image;
#[cfg(feature = "image")]
pub use ratatui_image;

// Modules that arrived with the layout and text-entry work package. They are
// appended here rather than sorted in among the rest so that the merge with
// whatever else was landing at the same time is a single hunk.
pub mod dock;
pub mod vlist;
pub mod wrap;
