//! Terminal engine layer. Ghostty owns all terminal semantics; this module
//! only owns the handles and translates data into plain Rust values.

pub mod ghostty;

pub mod fixtures;
pub mod geometry;
pub mod input;
pub mod session;
pub mod writer;

#[cfg(feature = "shell")]
pub mod render;

pub use ghostty::{
    paste_encode, paste_is_safe, Cell, Colors, Cursor, GhosttyError, GhosttyRenderState,
    GhosttyTerminal, KeyEncoder, KeyEvent, MouseEncoder, MouseEvent, RenderSnapshot, RowSelection,
    Style, WritePtyRegistration,
};
pub use session::{CountersSnapshot, TerminalSession, WakeupSink};
pub use writer::{WriterQueue, WriterStats};
