//! Pain: a minimal Windows/Linux terminal.
//!
//! Terminal semantics live in `pain-terminal-core`. This crate owns the
//! application: shell/profile policy, preferences, Silvestris composition and
//! the GPUI surface. PTY transport and the GPUI view are extracted into their
//! own crates after the terminal boundary is clean.

#[cfg(feature = "shell")]
pub mod app;
pub mod config;
#[cfg(feature = "shell")]
pub mod gpui;
pub mod log;
pub mod platform;
#[cfg(feature = "shell")]
pub mod preferences;
#[cfg(feature = "shell")]
pub mod shortcuts;
pub use pain_terminal_core as terminal;
pub mod version;
