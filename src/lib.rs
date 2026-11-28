//! Pain: a minimal Windows/Linux terminal.
//!
//! This crate is a consumer of three reusable libraries: `pain-terminal-core`
//! (terminal semantics), `pain-terminal-pty` (OS transport) and
//! `pain-terminal-gpui` (the GPUI surface). It owns the application: shell/
//! profile policy, preferences, Silvestris composition and window wiring.
//!
//! There is no compatibility façade here. Sandcat and other hosts consume the
//! `pain-terminal-*` crates directly; Pain only reexports its own fixtures.

#[cfg(feature = "shell")]
pub mod app;
pub mod config;
pub mod fixtures;
pub mod log;
pub mod platform;
#[cfg(feature = "shell")]
pub mod preferences;
#[cfg(feature = "shell")]
pub mod shortcuts;
pub mod version;
