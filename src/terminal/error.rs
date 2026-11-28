//! Stable terminal-domain error type. Consumers of the terminal API should not
//! need to know the raw Ghostty result constants or the platform writer
//! details; both are carried as internal variants.

use std::fmt;

use crate::terminal::ghostty::GhosttyError;
use crate::terminal::writer::WriterQueueError;

/// A recoverable failure reported by terminal-session operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalError {
    /// A Ghostty call failed. The raw code is intentionally wrapped rather
    /// than exposed as the public error contract.
    Ghostty(GhosttyError),
    /// Outbound input could not be enqueued for the platform writer.
    Writer(WriterQueueError),
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ghostty(error) => write!(f, "{error}"),
            Self::Writer(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TerminalError {}

impl From<GhosttyError> for TerminalError {
    fn from(error: GhosttyError) -> Self {
        Self::Ghostty(error)
    }
}

impl From<WriterQueueError> for TerminalError {
    fn from(error: WriterQueueError) -> Self {
        Self::Writer(error)
    }
}
