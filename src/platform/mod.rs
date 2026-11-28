//! Platform PTY transport. This layer owns only process/PTY mechanics and
//! byte movement; it never parses ANSI, shapes text, or handles GPUI events.
//! The common terminal layer talks to it through `PlatformPty`.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::terminal::session::TerminalSession;

#[cfg(target_os = "linux")]
pub mod linux;
pub mod profiles;
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
    /// Device-pixel cell size. `TIOCSWINSZ` carries these as `ws_xpixel` and
    /// `ws_ypixel` so Kitty clients can read the terminal's pixel geometry.
    pub cell_width_px: u32,
    pub cell_height_px: u32,
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

/// A host-relevant transport transition. This is deliberately a small,
/// closed set: internal worker bookkeeping is not an event. The host owns
/// presentation (logging, UI) and receives each fact exactly once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PtyEvent {
    /// The outbound writer can no longer reach the OS PTY.
    WriterFailed(PtyError),
    /// The reader failed while the child was still running (shutdown and
    /// expected cancellation are not reported as failures).
    ReaderFailed(PtyError),
    /// A resize accepted by the mailbox failed in the OS call.
    ResizeFailed(PtyError),
    /// The child exited. The code is `None` when the platform cannot read it.
    ProcessExited { exit_code: Option<u32> },
}

impl fmt::Display for PtyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WriterFailed(error) => write!(f, "PTY writer failed: {error}"),
            Self::ReaderFailed(error) => write!(f, "PTY reader failed: {error}"),
            Self::ResizeFailed(error) => write!(f, "PTY resize failed: {error}"),
            Self::ProcessExited { exit_code } => match exit_code {
                Some(code) => write!(f, "child process exited with code {code}"),
                None => write!(f, "child process exited"),
            },
        }
    }
}

/// One narrow sink supplied when a PTY is spawned. It is not a general event
/// bus: there is one host per PTY, and it decides what to do with each fact.
pub type PtyEventSink = Arc<dyn Fn(PtyEvent) + Send + Sync>;

/// Point-in-time counters for [`PtyResizeMailbox`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PtyResizeCounters {
    pub requested: u64,
    pub coalesced: u64,
    pub applied: u64,
    /// OS resize calls that failed. A failed resize is never counted as
    /// applied or coalesced.
    pub failed: u64,
}

/// A latest-value mailbox for OS PTY resizes. A drag from 80→200 columns may
/// enqueue many intermediate sizes; only the newest survives. One worker
/// thread applies it with the platform's `ResizePseudoConsole`/`TIOCSWINSZ`.
/// Nothing here touches Ghostty geometry, which must stay synchronous with the
/// frame.
struct PtyResizeState {
    pending: Mutex<Option<(u16, u16, u32, u32)>>,
    wake: Condvar,
    stopping: AtomicBool,
}

pub struct PtyResizeMailbox {
    state: Arc<PtyResizeState>,
    worker: Mutex<Option<JoinHandle<()>>>,
    requested: Arc<AtomicU64>,
    applied: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
}

impl PtyResizeMailbox {
    /// `apply` runs on the worker and reports the real OS result. Failures go
    /// to `events` once; the mailbox never logs them itself.
    #[must_use]
    pub fn new(
        apply: Arc<dyn Fn(u16, u16, u32, u32) -> Result<(), PtyError> + Send + Sync>,
        events: PtyEventSink,
    ) -> Self {
        let state = Arc::new(PtyResizeState {
            pending: Mutex::new(None),
            wake: Condvar::new(),
            stopping: AtomicBool::new(false),
        });
        let requested = Arc::new(AtomicU64::new(0));
        let applied = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicU64::new(0));
        let worker_state = Arc::clone(&state);
        let worker_applied = Arc::clone(&applied);
        let worker_failed = Arc::clone(&failed);
        let worker = std::thread::Builder::new()
            .name("pain-pty-resize".into())
            .spawn(move || {
                let mut last_applied: Option<(u16, u16, u32, u32)> = None;
                loop {
                    let mut pending = worker_state
                        .pending
                        .lock()
                        .expect("PTY resize pending mutex poisoned");
                    while pending.is_none() && !worker_state.stopping.load(Ordering::Acquire) {
                        pending = worker_state
                            .wake
                            .wait(pending)
                            .expect("PTY resize condvar poisoned");
                    }
                    if worker_state.stopping.load(Ordering::Acquire) {
                        break;
                    }
                    let size = pending.take().expect("pending size was checked");
                    drop(pending);
                    // Shutdown can race a pending request: never apply a resize
                    // (or emit a success) once stopping has begun.
                    if worker_state.stopping.load(Ordering::Acquire) {
                        break;
                    }
                    // Worker-side dedup: never apply the same grid twice.
                    if last_applied == Some(size) {
                        continue;
                    }
                    match apply(size.0, size.1, size.2, size.3) {
                        Ok(()) => {
                            last_applied = Some(size);
                            worker_applied.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => {
                            worker_failed.fetch_add(1, Ordering::Relaxed);
                            events(PtyEvent::ResizeFailed(error));
                        }
                    }
                }
            })
            .expect("spawn PTY resize worker");
        Self {
            state,
            worker: Mutex::new(Some(worker)),
            requested,
            applied,
            failed,
        }
    }

    /// Update only the pending size and signal the worker. Never blocks on the
    /// OS PTY call, so this is safe to invoke from the UI/session path. The
    /// result reports only whether the request was accepted by the mailbox;
    /// the OS result arrives later as a [`PtyEvent`].
    pub fn request(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), PtyError> {
        if self.state.stopping.load(Ordering::Acquire) {
            return Err(PtyError::new(
                "request_resize",
                "resize worker is shutting down",
            ));
        }
        let size = (cols.max(1), rows.max(1), cell_width_px, cell_height_px);
        self.requested.fetch_add(1, Ordering::Relaxed);
        let mut pending = self
            .state
            .pending
            .lock()
            .expect("PTY resize pending mutex poisoned");
        if *pending == Some(size) {
            return Ok(());
        }
        *pending = Some(size);
        drop(pending);
        self.state.wake.notify_one();
        Ok(())
    }

    /// Stop accepting work, wake the worker, and join it. Idempotent.
    pub fn shutdown(&self) {
        // Hold `pending` while flipping `stopping`: the worker checks that flag
        // under the same mutex before it waits. Setting it without the lock
        // would allow a notify in the window between the worker's check and
        // `Condvar::wait`, losing the wakeup and hanging `join()` forever.
        {
            let _pending = self
                .state
                .pending
                .lock()
                .expect("PTY resize pending mutex poisoned");
            self.state.stopping.store(true, Ordering::Release);
        }
        self.state.wake.notify_all();
        if let Some(worker) = self
            .worker
            .lock()
            .expect("PTY resize worker mutex poisoned")
            .take()
        {
            let _ = worker.join();
        }
    }

    #[must_use]
    pub fn counters(&self) -> PtyResizeCounters {
        let requested = self.requested.load(Ordering::Relaxed);
        let applied = self.applied.load(Ordering::Relaxed);
        let failed = self.failed.load(Ordering::Relaxed);
        PtyResizeCounters {
            requested,
            // Every request is applied, failed in the OS call, or coalesced
            // away because a newer grid superseded it first.
            coalesced: requested.saturating_sub(applied).saturating_sub(failed),
            applied,
            failed,
        }
    }
}

/// Owns one live PTY/child and its worker threads. `shutdown` is the only
/// blocking call and is always made off the GPUI foreground thread.
pub trait PlatformPty: Send {
    /// Request a grid resize. The implementation coalesces duplicate sizes and
    /// carries the current device-pixel cell size into the OS PTY winsize.
    /// The immediate result says only whether the request was accepted by the
    /// mailbox; the real OS call happens on the worker and its failure arrives
    /// through [`PtyEvent::ResizeFailed`]. Never blocks on the OS call.
    fn request_resize(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), PtyError>;
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
    events: PtyEventSink,
) -> Result<Box<dyn PlatformPty>, PtyError> {
    linux::LinuxPty::spawn(config, session, events).map(|pty| Box::new(pty) as Box<dyn PlatformPty>)
}

#[cfg(target_os = "windows")]
pub fn spawn_pty(
    config: &PtyConfig,
    session: Arc<TerminalSession>,
    events: PtyEventSink,
) -> Result<Box<dyn PlatformPty>, PtyError> {
    windows::WindowsPty::spawn(config, session, events)
        .map(|pty| Box::new(pty) as Box<dyn PlatformPty>)
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
    fn resize_mailbox_keeps_only_the_latest_grid() {
        use std::time::{Duration, Instant};

        let applied_grids = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&applied_grids);
        // Gate the worker until every request is queued. Without this the
        // worker can legitimately apply an intermediate drag size, making an
        // otherwise-correct mailbox look like it failed to coalesce.
        let release = Arc::new(AtomicBool::new(false));
        let worker_release = Arc::clone(&release);
        let mailbox = PtyResizeMailbox::new(
            Arc::new(move |cols, rows, _cw, _ch| {
                while !worker_release.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                seen.lock().expect("seen poisoned").push((cols, rows));
                Ok(())
            }),
            Arc::new(|_| {}),
        );
        for cols in 80..200u16 {
            mailbox
                .request(cols, 24, 10, 20)
                .expect("resize request accepted");
        }
        mailbox
            .request(200, 24, 10, 20)
            .expect("final resize request accepted");
        release.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && mailbox.counters().applied == 0 {
            std::thread::yield_now();
        }
        mailbox.shutdown();
        let grids = applied_grids.lock().expect("seen poisoned");
        assert_eq!(grids.last(), Some(&(200, 24)), "child ends at latest grid");
        assert!(grids.len() <= 2, "intermediate drag sizes are coalesced");
        let counters = mailbox.counters();
        assert_eq!(counters.requested, 121);
        assert_eq!(counters.coalesced + counters.applied, 121);
    }

    #[test]
    fn resize_application_failure_is_an_event_not_a_success() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&events);
        let sink_events = Arc::clone(&events);
        let sink: PtyEventSink = Arc::new(move |event| {
            sink_events.lock().expect("events poisoned").push(event);
        });
        let mailbox = PtyResizeMailbox::new(
            Arc::new(|_, _, _, _| Err(PtyError::new("ResizePseudoConsole", "Access denied"))),
            sink,
        );
        mailbox
            .request(100, 30, 10, 20)
            .expect("request accepted by mailbox");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && seen.lock().expect("events poisoned").is_empty()
        {
            std::thread::yield_now();
        }
        mailbox.shutdown();
        let counters = mailbox.counters();
        assert_eq!(counters.failed, 1);
        assert_eq!(counters.applied, 0);
        assert_eq!(counters.coalesced, 0);
        let seen = seen.lock().expect("events poisoned");
        assert_eq!(seen.len(), 1, "failure delivered once");
        assert!(matches!(
            seen.first(),
            Some(PtyEvent::ResizeFailed(error)) if error.operation == "ResizePseudoConsole"
        ));
    }

    #[test]
    fn request_resize_reports_mailbox_shutdown() {
        let mailbox = PtyResizeMailbox::new(Arc::new(|_, _, _, _| Ok(())), Arc::new(|_| {}));
        mailbox.shutdown();
        assert!(mailbox.request(80, 24, 0, 0).is_err());
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
