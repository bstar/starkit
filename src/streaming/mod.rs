//! Original-file HTTP byte ranges for native players, carried inside SSH.
//!
//! This module has no codec, audio-device, terminal, or application dependencies.
//! Catalog documents belong to the application; all media URLs use opaque IDs.
mod server;
pub mod ssh;
pub use server::{Resource, Server, Session};
