//! Terminal semantics and Ghostty adaptation.
//!
//! This crate owns terminal parsing state, input translation, geometry,
//! instrumentation and the outbound byte queue. It does **not** know about
//! GPUI, PTYs, shells, preferences, logging frameworks or any application.
//! Ghostty remains authoritative for the screen, scrollback, modes and
//! selection; this crate only translates between Ghostty and plain Rust data.

pub mod counters;
pub mod error;
pub mod ghostty;
pub mod session;
pub mod writer;

pub mod fixtures;
pub mod geometry;
pub mod input;
pub mod palette;
pub mod selection;

// Narrow public surface: consumers get terminal-domain values and session
// operations, never raw Ghostty handles or C constants.
pub use error::TerminalError;
pub use ghostty::{
    paste_encode, paste_is_safe, Cell, Colors, Cursor, FrameMeta, ImagePlacement,
    ImagePlacementCache, ImagePlacementKind, KeyEvent, KittyConfig, KittyImagePixels, RenderFrame,
    RenderSnapshot, RowSelection, ScrollbarState, Style,
};
pub use session::{
    AppliedGeometry, CountersHandle, CountersSnapshot, MouseInput, MouseRoute, ResizeOutcome,
    ScrollOutcome, SessionConfig, TerminalSession, WakeupSink,
};
pub use writer::{WriterQueue, WriterQueueError, WriterQueueState, WriterStats};
