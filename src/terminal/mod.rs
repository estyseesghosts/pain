//! Terminal engine layer. Ghostty owns all terminal semantics; this module
//! only owns the handles and translates data into plain Rust values.

pub mod error;
pub mod ghostty;

pub mod fixtures;
pub mod geometry;
pub mod input;
pub mod palette;
pub mod selection;
pub mod session;
pub mod writer;

#[cfg(feature = "shell")]
pub mod images;
#[cfg(feature = "shell")]
pub mod render;

// Narrow public surface: consumers get terminal-domain values and session
// operations, never raw Ghostty handles or C constants.
pub use error::TerminalError;
pub use ghostty::{
    paste_encode, paste_is_safe, Cell, Colors, Cursor, FrameMeta, ImagePlacement,
    ImagePlacementCache, ImagePlacementKind, KeyEvent, KittyConfig, KittyImagePixels, RenderFrame,
    RenderSnapshot, RowSelection, ScrollbarState, Style,
};
pub use session::{
    AppliedGeometry, CountersSnapshot, MouseInput, MouseRoute, ResizeOutcome, ScrollOutcome,
    SessionConfig, TerminalSession, WakeupSink,
};
pub use writer::{WriterQueue, WriterQueueError, WriterQueueState, WriterStats};
