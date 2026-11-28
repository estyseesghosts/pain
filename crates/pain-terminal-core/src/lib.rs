//! Terminal semantics and Ghostty adaptation.
//!
//! This crate owns terminal parsing state, input translation, geometry,
//! instrumentation and the outbound byte queue. It does **not** know about
//! GPUI, PTYs, shells, preferences, logging frameworks or any application.
//! Ghostty remains authoritative for the screen, scrollback, modes and
//! selection; this crate only translates between Ghostty and plain Rust data.
//!
//! Implementation modules are private on purpose. The root reexports below
//! are the integration contract for `pain-terminal-gpui`, `pain-terminal-pty`
//! and host applications. Ghostty constants, queue machinery and application
//! fixtures are not part of it.

mod counters;
mod error;
mod geometry;
mod ghostty;
mod input;
mod palette;
mod selection;
mod session;
mod writer;

pub use error::TerminalError;

pub use session::{
    AppliedGeometry, CountersHandle, CountersSnapshot, MouseInput, MouseRoute, NoopWakeup,
    ResizeOutcome, ScrollOutcome, SessionConfig, TerminalSession, WakeupSink,
};
pub use writer::{WriterQueueError, WriterQueueState, WriterStats};

pub use geometry::{
    adjust_font_size, background_runs, batch_row, display_columns, line_height_for, selection_span,
    BgRun, CellMetrics, InputGeometry, PointerSample, RowCache, StyleKey, TerminalGeometry,
    TextRun, FONT_SIZE_STEP, MAX_FONT_SIZE, MIN_FONT_SIZE,
};
pub use ghostty::{
    paste_encode, paste_is_safe, version, Cell, ColorSpec, Colors, Cursor, FrameMeta, GhosttyError,
    ImageLayer, ImagePlacement, ImagePlacementCache, ImagePlacementKind, ImageProtocol, KeyEvent,
    KittyConfig, KittyImagePixels, KittyPixelFormat, RenderFrame, RenderSnapshot, RowSelection,
    ScrollbarState, Style,
};
pub use input::{
    key_for_keystroke, key_for_name, key_release_for_keystroke, mouse_escape, should_commit_text,
    EncodedKey, KeyInput, Modifiers, MouseAction, MouseButton,
};
pub use palette::{resolve, Palette, PaletteChoice, Rgb};
pub use selection::{
    accumulate_wheel, scrollbar_geometry, scrollbar_row_at, wheel_button, wheel_to_ghostty_delta,
    ScrollbarGeometry,
};

// Keyboard input still carries a Ghostty-shaped `KeyEvent` and `EncodedKey.key`
// is a numeric key code. That is a documented follow-up for a keyboard-domain
// API redesign; mouse input already uses the typed `MouseInput`/`PointerSample`
// contract and must not be dragged back into raw scalar parameters.
