//! Pain: a minimal Windows/Linux terminal.
//!
//! This crate is a consumer of three reusable libraries: `pain-terminal-core`
//! (terminal semantics), `pain-terminal-pty` (OS transport) and
//! `pain-terminal-gpui` (the GPUI surface). It owns the application: shell/
//! profile policy, preferences, Silvestris composition and window wiring.

#[cfg(feature = "shell")]
pub mod app;
pub mod config;
pub mod log;
pub mod platform;
#[cfg(feature = "shell")]
pub mod preferences;
#[cfg(feature = "shell")]
pub mod shortcuts;
pub mod version;

// Backward-compatible aliases: integration gates import terminal values and
// the view through Pain. The implementations live in the reusable crates.
pub use pain_terminal_core as terminal;
#[cfg(feature = "shell")]
pub use pain_terminal_gpui as gpui;
