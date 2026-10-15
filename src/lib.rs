//! A native terminal application framework for cell, graphical and SSH media presentation.
//!
//! STAR/AMP, STAR/FOLD, STAR/CORD and STAR/WIRE exercise the shared foundation.
//! The framework is being prepared for public use; APIs remain versioned 0.x.
//! Applications retain their domain models while KIT owns presentation,
//! terminal capabilities, shared interaction and media transport.
//!
//! ## One copy of ratatui
//!
//! `ratatui`, `crossterm` and -- with the `image` feature -- `ratatui_image`
//! and `image` are re-exported from here, and applications reach them
//! through this crate rather than depending on them directly. A `Widget`
//! implemented against a second copy of ratatui does not satisfy a signature
//! expecting the first, and the compiler reports that as a mismatch between
//! two versions carrying the same number, which sends the reader looking for a
//! problem that is not there.

pub mod chrome;
pub mod config;
pub mod content_view;
pub mod diagnostics;
pub mod digits;
pub mod editor_controls;
pub mod fs;
#[cfg(feature = "image")]
pub mod graphics;
pub mod keymap;
pub mod list;
pub mod logging;
pub mod media_controls;
pub mod media_types;
pub mod mouse;
// An HTTP client is not something a UI library should force on a caller who
// only wanted a theme engine.
#[cfg(feature = "net")]
pub mod net;
pub mod panes;
pub mod paths;
pub mod presentation;
pub mod term;
pub mod text;
pub mod theme;

pub use crossterm;
pub use ratatui;

#[cfg(feature = "image")]
pub use image;
#[cfg(feature = "image")]
pub use ratatui_image;
// Re-exported for the same reason as the rest: `net::builder` hands back one of
// this crate's types, and a caller holding a second copy of it cannot use it.
#[cfg(feature = "net")]
pub use ureq;

// Modules that arrived with the layout and text-entry work package. They are
// appended here rather than sorted in among the rest so that the merge with
// whatever else was landing at the same time is a single hunk.
#[cfg(feature = "gif")]
pub mod anim;
#[cfg(feature = "gif")]
pub mod animation;
pub mod dock;
pub mod input;
pub mod vlist;
pub mod worker;
pub mod wrap;

#[cfg(feature = "terminal-graphics")]
pub mod terminal_graphics;

/// Native surface contract, usable by helpers without linking the renderer.
pub mod native_surface;

#[cfg(feature = "media")]
pub mod media;

/// Verified standalone application updates from GitHub releases.
#[cfg(all(feature = "update", unix))]
pub mod update;

/// Lossless PCM stream metadata and precision conversion.
pub mod pcm;

#[cfg(feature = "audio-output")]
pub mod pcm_output;
