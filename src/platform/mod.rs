//! Application-owned shell policy and profile discovery. Transport mechanics
//! live in `pain-terminal-pty`; this module only chooses which program to run.

use std::path::PathBuf;

// Only the transport values the application itself needs. Backend internals
// and integration-test helpers live in `pain-terminal-pty` and are imported
// there directly by the tests that use them.
pub use pain_terminal_pty::{
    spawn_pty, PlatformPty, PtyConfig, PtyError, PtyEvent, PtyEventSink, ShellCommand,
};

pub mod profiles;

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

    #[cfg(target_os = "linux")]
    #[test]
    fn default_shell_is_absolute_when_set() {
        let shell = default_shell();
        assert!(shell.program.starts_with('/'), "{}", shell.program);
        assert!(shell.args.is_empty());
    }
}
