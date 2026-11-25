//! Platform PTY transport. This layer owns only process/PTY mechanics and
//! byte movement; it never parses ANSI, shapes text, or handles GPUI events.
//! The common terminal layer talks to it through `PlatformPty`.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::terminal::session::TerminalSession;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "windows")]
pub mod windows;

/// Explicit shell command selected by CLI/config. No auto-discovery matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ShellCommand {
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }
}

/// One PTY session's creation parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyConfig {
    pub cols: u16,
    pub rows: u16,
    pub command: ShellCommand,
}

/// Concise platform error with operation context. Native error codes are
/// kept as text at the boundary instead of being stringified deep in FFI code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PtyError {
    pub operation: &'static str,
    pub detail: String,
}

impl PtyError {
    #[must_use]
    pub fn new(operation: &'static str, detail: impl Into<String>) -> Self {
        Self {
            operation,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PtyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation, self.detail)
    }
}

impl std::error::Error for PtyError {}

/// Owns one live PTY/child and its worker threads. `shutdown` is the only
/// blocking call and is always made off the GPUI foreground thread.
pub trait PlatformPty: Send {
    /// Request a grid resize. The implementation coalesces duplicate sizes.
    fn resize(&self, cols: u16, rows: u16) -> Result<(), PtyError>;
    #[must_use]
    fn process_exited(&self) -> bool;
    /// Close the PTY, unblock workers, reap the child and join threads.
    fn shutdown(self: Box<Self>);
}

/// Launch the platform backend and start reader/writer/waiter threads. Each
/// output chunk goes through `TerminalSession::write_vt` (the same serialized
/// path the M2 synthetic producer uses).
#[cfg(target_os = "linux")]
pub fn spawn_pty(
    config: &PtyConfig,
    session: Arc<TerminalSession>,
) -> Result<Box<dyn PlatformPty>, PtyError> {
    linux::LinuxPty::spawn(config, session).map(|pty| Box::new(pty) as Box<dyn PlatformPty>)
}

#[cfg(target_os = "windows")]
pub fn spawn_pty(
    config: &PtyConfig,
    session: Arc<TerminalSession>,
) -> Result<Box<dyn PlatformPty>, PtyError> {
    windows::WindowsPty::spawn(config, session).map(|pty| Box::new(pty) as Box<dyn PlatformPty>)
}

/// Conservative terminal identity shared by both backends. We do not claim
/// `xterm-ghostty` because this renderer deliberately omits Ghostty protocols.
pub fn terminal_environment() -> [(&'static str, &'static str); 2] {
    [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]
}

/// `$SHELL` when it is an executable absolute path, otherwise `/bin/sh`.
/// Explicit CLI commands take precedence and are handled before this is used.
#[cfg(target_os = "linux")]
#[must_use]
pub fn default_shell() -> ShellCommand {
    if let Some(shell) = std::env::var_os("SHELL") {
        let path = PathBuf::from(&shell);
        if path.is_absolute() && path.is_file() {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&path) {
                if meta.permissions().mode() & 0o111 != 0 {
                    return ShellCommand::new(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    ShellCommand::new("/bin/sh")
}

/// Conservative Windows default: the in-box command interpreter.
#[cfg(target_os = "windows")]
#[must_use]
pub fn default_shell() -> ShellCommand {
    if let Some(comspec) = std::env::var_os("ComSpec") {
        let path = PathBuf::from(&comspec);
        if path.is_absolute() && path.is_file() {
            return ShellCommand::new(path.to_string_lossy().into_owned());
        }
    }
    ShellCommand::new(r"C:\Windows\System32\cmd.exe")
}

/// Parse `--shell <program> [args...]` from `argv`. Everything after a bare
/// `--` is also treated as the shell command line. Invalid/empty input falls
/// back to `None` so the platform default applies.
#[must_use]
pub fn shell_from_args(args: &[String]) -> Option<ShellCommand> {
    if let Some(index) = args.iter().position(|arg| arg == "--shell") {
        let mut rest = args[index + 1..].iter();
        let program = rest.next()?.clone();
        if program.is_empty() {
            return None;
        }
        return Some(ShellCommand {
            program,
            args: rest.cloned().collect(),
        });
    }
    if let Some(index) = args.iter().position(|arg| arg == "--") {
        let mut rest = args[index + 1..].iter();
        let program = rest.next()?.clone();
        if program.is_empty() {
            return None;
        }
        return Some(ShellCommand {
            program,
            args: rest.cloned().collect(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_override_from_args() {
        let args = vec![
            "pain".into(),
            "--shell".into(),
            "powershell.exe".into(),
            "-NoLogo".into(),
        ];
        assert_eq!(
            shell_from_args(&args),
            Some(ShellCommand {
                program: "powershell.exe".into(),
                args: vec!["-NoLogo".into()],
            })
        );
        let args = vec!["pain".into(), "--".into(), "/bin/bash".into(), "-i".into()];
        assert_eq!(
            shell_from_args(&args),
            Some(ShellCommand {
                program: "/bin/bash".into(),
                args: vec!["-i".into()],
            })
        );
        assert_eq!(shell_from_args(&["pain".into()]), None);
        assert_eq!(shell_from_args(&["pain".into(), "--shell".into()]), None);
    }

    #[test]
    fn conservative_terminal_identity() {
        assert_eq!(
            terminal_environment(),
            [("TERM", "xterm-256color"), ("COLORTERM", "truecolor")]
        );
    }

    #[test]
    fn pty_error_keeps_operation_context() {
        let error = PtyError::new("CreatePseudoConsole", "HRESULT 0x80070005");
        assert_eq!(error.to_string(), "CreatePseudoConsole: HRESULT 0x80070005");
    }
}
