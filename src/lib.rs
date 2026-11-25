//! Pain: a minimal Windows/Linux terminal.
//!
//! M0 proved the `libghostty-vt` dependency from Rust. M1 adds a static GPUI
//! terminal element inside one Silvestris window; M2 adds the shared session,
//! two-phase render update and coalesced wakeup model. Platform PTY transport
//! (M3/M4) is not part of this build.

#[cfg(feature = "shell")]
pub mod app;
pub mod config;
pub mod log;
pub mod platform;
pub mod terminal;
