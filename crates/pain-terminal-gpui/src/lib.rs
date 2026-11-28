//! Reusable GPUI terminal surface.
//!
//! This crate owns the drawing and input-events side of a terminal: the
//! `TerminalView`, its custom GPUI element, the image texture cache, cursor /
//! animation / IME state, scrollbar rendering and intrinsic actions. It
//! depends only on `pain-terminal-core` and GPUI. It deliberately knows
//! nothing about PTYs, shells, preferences, logging frameworks, Silvestris or
//! Pain: the host owns all of those and supplies the callbacks it needs.
//!
//! `TerminalSession` remains authoritative for terminal semantics. GPUI cache
//! invalidation is driven entirely by the session's generations and dirty
//! state.

mod images;
mod render;

// Narrow public surface. Cached rows, animation epochs, pointer-capture
// implementation, `TerminalInputHandler` and the image cache stay private.
pub use render::{
    CopySelection, DecreaseFont, GridChangeSink, IncreaseFont, PasteClipboard, ResetFont,
    SelectAllTerminal, ShortcutFilter, TerminalErrorSink, TerminalFont, TerminalView,
};
