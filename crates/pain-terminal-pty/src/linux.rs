//! M4 POSIX PTY backend. Only `libc` PTY/process primitives live here: no GTK,
//! no Wayland/X11, no ANSI parsing. GPUI/Silvestris own the window; this module
//! owns one child, its controlling terminal, and three worker threads.

use std::io;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use libc::{c_int, pid_t};

use super::{
    terminal_environment, PlatformPty, PtyConfig, PtyError, PtyEvent, PtyEventSink,
    PtyResizeMailbox,
};
use pain_terminal_core::TerminalSession;

/// `TIOCSWINSZ` payload derived from the terminal grid. The pixel fields stay
/// zero until the renderer has measured a cell; they are carried only when the
/// grid itself changes, so a DPI-only update never reaches this backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridSize {
    cols: u16,
    rows: u16,
    cell_width_px: u32,
    cell_height_px: u32,
}

#[must_use]
fn winsize_for(size: GridSize) -> libc::winsize {
    let pixels = |cells: u16, cell: u32| -> u16 {
        u32::from(cells)
            .saturating_mul(cell.max(1))
            .min(u32::from(u16::MAX)) as u16
    };
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: pixels(size.cols, size.cell_width_px),
        ws_ypixel: pixels(size.rows, size.cell_height_px),
    }
}

/// Owning master FD. `close` is called once from `Drop`; `shutdown` does not
/// close it because a concurrent `poll`/`read` could otherwise race on reuse.
struct MasterFd {
    fd: AtomicI32,
}

impl MasterFd {
    #[must_use]
    fn new(fd: RawFd) -> Self {
        Self {
            fd: AtomicI32::new(fd),
        }
    }

    fn raw(&self) -> RawFd {
        self.fd.load(Ordering::Acquire)
    }

    /// Wait for readability. Returns `false` on timeout so the caller can
    /// re-check its shutdown flag without a blocking close/read race.
    fn wait_readable(&self, timeout_ms: c_int) -> io::Result<bool> {
        let mut pollfd = libc::pollfd {
            fd: self.raw(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let result = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Ok(result > 0);
        }
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let result = unsafe { libc::read(self.raw(), buf.as_mut_ptr().cast(), buf.len()) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            return Ok(result as usize);
        }
    }

    fn write_all(&self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let result = unsafe { libc::write(self.raw(), bytes.as_ptr().cast(), bytes.len()) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if result == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "PTY master accepted zero bytes",
                ));
            }
            bytes = &bytes[result as usize..];
        }
        Ok(())
    }
}

impl Drop for MasterFd {
    fn drop(&mut self) {
        let fd = self.fd.swap(-1, Ordering::AcqRel);
        if fd >= 0 {
            // SAFETY: this wrapper owned the descriptor and swapped it out, so
            // this is the single close of it.
            unsafe { libc::close(fd) };
        }
    }
}

struct Threads {
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    waiter: JoinHandle<()>,
}

/// Owns one POSIX PTY, its child, and its worker threads.
pub struct LinuxPty {
    _master: Arc<MasterFd>,
    child: pid_t,
    stop: Arc<AtomicBool>,
    /// Process lifecycle belongs to the PTY layer, not the terminal session.
    exited: Arc<AtomicBool>,
    session: Arc<TerminalSession>,
    _size: Arc<Mutex<GridSize>>,
    /// Latest-value mailbox; the UI only updates the pending grid.
    resize_mailbox: Arc<PtyResizeMailbox>,
    threads: Mutex<Option<Threads>>,
}

impl LinuxPty {
    pub fn spawn(
        config: &PtyConfig,
        session: Arc<TerminalSession>,
        events: PtyEventSink,
    ) -> Result<Self, PtyError> {
        let mut master_fd: RawFd = -1;
        let mut slave_fd: RawFd = -1;
        let initial = GridSize {
            cols: config.cols.max(1),
            rows: config.rows.max(1),
            cell_width_px: config.cell_width_px,
            cell_height_px: config.cell_height_px,
        };
        // SAFETY: openpty writes two valid fds into our locals; all pointers
        // are either valid or NULL with the documented meaning.
        let result = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null(),
                &winsize_for(initial),
            )
        };
        if result != 0 {
            return Err(PtyError::new(
                "openpty",
                io::Error::last_os_error().to_string(),
            ));
        }
        // SAFETY: both fds came from openpty and are exclusively owned here.
        unsafe {
            let flags = libc::fcntl(master_fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(master_fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
            // UTF-8 line editing in the line discipline, where supported.
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(slave_fd, &mut termios) == 0 {
                termios.c_iflag |= libc::IUTF8;
                libc::tcsetattr(slave_fd, libc::TCSANOW, &termios);
            }
        }

        let mut command = std::process::Command::new(&config.command.program);
        command.args(&config.command.args);
        for (key, value) in terminal_environment() {
            command.env(key, value);
        }
        let child_master = master_fd;
        let child_slave = slave_fd;
        // SAFETY: the pre-exec closure runs after fork in the child. It calls
        // only async-signal-safe libc primitives; no allocation, locks or logs.
        unsafe {
            command.pre_exec(move || {
                libc::setsid();
                libc::ioctl(child_slave, libc::TIOCSCTTY, 0);

                // Reset signals the shell/TUI expects to control itself.
                for signal in [
                    libc::SIGPIPE,
                    libc::SIGINT,
                    libc::SIGQUIT,
                    libc::SIGTERM,
                    libc::SIGHUP,
                    libc::SIGCHLD,
                    libc::SIGTSTP,
                    libc::SIGTTIN,
                    libc::SIGTTOU,
                ] {
                    libc::signal(signal, libc::SIG_DFL);
                }

                if libc::dup2(child_slave, 0) < 0
                    || libc::dup2(child_slave, 1) < 0
                    || libc::dup2(child_slave, 2) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                if child_slave > 2 {
                    libc::close(child_slave);
                }
                // The child must never retain the master: that would prevent
                // the parent from observing EOF when the shell exits.
                libc::close(child_master);
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|error| PtyError::new("spawn shell", error.to_string()))?;
        // SAFETY: the parent's slave copy is no longer needed; closing it is
        // what lets the master see EOF when the child exits.
        unsafe { libc::close(slave_fd) };

        let master = Arc::new(MasterFd::new(master_fd));
        let stop = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let threads = match start_threads(
            &session,
            Arc::clone(&master),
            Arc::clone(&stop),
            Arc::clone(&exited),
            child.id(),
            Arc::clone(&events),
        ) {
            Ok(threads) => threads,
            Err(error) => {
                // Workers are torn down before returning; kill the child's
                // private process group so it cannot outlive a failed spawn.
                // SAFETY: negative pid targets only our own setsid group.
                unsafe {
                    libc::kill(-(child.id() as libc::pid_t), libc::SIGHUP);
                }
                return Err(error);
            }
        };

        let size = Arc::new(Mutex::new(initial));
        let resize_master = Arc::clone(&master);
        let resize_size = Arc::clone(&size);
        let resize_events = Arc::clone(&events);
        let resize_mailbox = Arc::new(PtyResizeMailbox::new(
            Arc::new(move |cols, rows, cell_width_px, cell_height_px| {
                let next = GridSize {
                    cols: cols.max(1),
                    rows: rows.max(1),
                    cell_width_px,
                    cell_height_px,
                };
                // SAFETY: the winsize value is POD and the master fd is alive until
                // shutdown joins this worker. Unlike ConPTY, TIOCSWINSZ carries
                // device-pixel dimensions for Kitty size-report clients.
                let result = unsafe {
                    libc::ioctl(resize_master.raw(), libc::TIOCSWINSZ, &winsize_for(next))
                };
                if result != 0 {
                    return Err(PtyError::new(
                        "TIOCSWINSZ",
                        io::Error::last_os_error().to_string(),
                    ));
                }
                *resize_size.lock().expect("PTY size mutex poisoned") = next;
                Ok(())
            }),
            resize_events,
        )?);
        Ok(Self {
            _master: master,
            child: child.id() as pid_t,
            stop,
            exited,
            session,
            _size: size,
            resize_mailbox,
            threads: Mutex::new(Some(threads)),
        })
    }
}

fn start_threads(
    session: &Arc<TerminalSession>,
    master: Arc<MasterFd>,
    stop: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    child: u32,
    events: PtyEventSink,
) -> Result<Threads, PtyError> {
    let reader_session = Arc::clone(session);
    let reader_master = Arc::clone(&master);
    let reader_stop = Arc::clone(&stop);
    let reader_events = Arc::clone(&events);
    let reader = thread::Builder::new()
        .name("pain-pty-reader".into())
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                if reader_stop.load(Ordering::Acquire) {
                    break;
                }
                match reader_master.wait_readable(100) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => {
                        if !reader_stop.load(Ordering::Acquire) {
                            reader_events(PtyEvent::ReaderFailed(PtyError::new(
                                "poll",
                                error.to_string(),
                            )));
                        }
                        break;
                    }
                }
                match reader_master.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => reader_session.write_vt(&buffer[..count]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        if !reader_stop.load(Ordering::Acquire) {
                            reader_events(PtyEvent::ReaderFailed(PtyError::new(
                                "read",
                                error.to_string(),
                            )));
                        }
                        break;
                    }
                }
            }
        });
    let reader = match reader {
        Ok(handle) => handle,
        Err(error) => return Err(PtyError::new("spawn PTY reader", error.to_string())),
    };

    let writer_session = Arc::clone(session);
    let writer_master = Arc::clone(&master);
    let writer_events = Arc::clone(&events);
    let writer = thread::Builder::new()
        .name("pain-pty-writer".into())
        .spawn(move || {
            loop {
                let bytes = match writer_session.wait_writer() {
                    Ok(Some(bytes)) => bytes,
                    // Normal shutdown: the accepted buffers are flushed.
                    Ok(None) => break,
                    // Ghostty's WRITE_PTY callback cannot call the host; the
                    // worker reports the terminal-side failure exactly once.
                    Err(error) => {
                        writer_events(PtyEvent::WriterFailed(PtyError::new(
                            "writer queue",
                            error.to_string(),
                        )));
                        break;
                    }
                };
                if let Err(error) = writer_master.write_all(&bytes) {
                    // The transport is gone: fail the queue so no later input,
                    // paste or Ghostty reply is accepted into a dead writer.
                    writer_session.fail_writer();
                    writer_events(PtyEvent::WriterFailed(PtyError::new(
                        "write",
                        error.to_string(),
                    )));
                    break;
                }
            }
        });
    let writer = match writer {
        Ok(handle) => handle,
        Err(error) => {
            // The poll loop checks `stop` at most 100 ms later; join it rather
            // than leaving a reader holding the master fd.
            stop.store(true, Ordering::Release);
            let _ = reader.join();
            return Err(PtyError::new("spawn PTY writer", error.to_string()));
        }
    };

    let waiter_exited = Arc::clone(&exited);
    let waiter_events = Arc::clone(&events);
    let waiter = thread::Builder::new()
        .name("pain-child-waiter".into())
        .spawn(move || {
            let pid = child as pid_t;
            let mut status = 0;
            // SAFETY: `pid` is our own direct child; reaping it exactly once
            // here prevents a zombie.
            unsafe { libc::waitpid(pid, &mut status, 0) };
            let exit_code = if libc::WIFEXITED(status) {
                Some(libc::WEXITSTATUS(status) as u32)
            } else {
                None
            };
            // The PTY owns process lifecycle state; the event tells the host.
            waiter_exited.store(true, Ordering::Release);
            waiter_events(PtyEvent::ProcessExited { exit_code });
        });
    let waiter = match waiter {
        Ok(handle) => handle,
        Err(error) => {
            stop.store(true, Ordering::Release);
            session.close_writer();
            let _ = reader.join();
            let _ = writer.join();
            return Err(PtyError::new("spawn child waiter", error.to_string()));
        }
    };

    Ok(Threads {
        reader,
        writer,
        waiter,
    })
}

impl PlatformPty for LinuxPty {
    fn request_resize(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), PtyError> {
        // TIOCSWINSZ runs on the coalescing worker, never on the UI thread.
        // The immediate result only reports mailbox acceptance; a real ioctl
        // failure arrives as PtyEvent::ResizeFailed.
        self.resize_mailbox
            .request(cols, rows, cell_width_px, cell_height_px)
    }

    fn process_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    fn shutdown(self: Box<Self>) -> Result<(), PtyError> {
        self.stop.store(true, Ordering::Release);
        // Stop accepting resizes, wake the worker and join before the master
        // fd can be dropped.
        self.resize_mailbox.shutdown();
        self.session.close_writer();
        // Signal the child's process group (created by setsid), then escalate
        // if it does not exit promptly so the waiter cannot hang forever.
        // SAFETY: the negative pid targets only our own new process group.
        unsafe {
            libc::kill(-self.child, libc::SIGHUP);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        // SAFETY: same process group as above.
        unsafe {
            libc::kill(-self.child, libc::SIGKILL);
        }
        // Join every worker even if one panicked; report the first panic once
        // the whole group has been torn down.
        let mut failure: Option<PtyError> = None;
        if let Some(threads) = self.threads.lock().expect("threads mutex poisoned").take() {
            for (name, worker) in [
                ("reader", threads.reader),
                ("writer", threads.writer),
                ("waiter", threads.waiter),
            ] {
                if worker.join().is_err() && failure.is_none() {
                    failure = Some(PtyError::new(
                        "join POSIX PTY worker",
                        format!("{name} worker panicked during shutdown"),
                    ));
                }
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winsize_reports_grid_and_device_pixels() {
        let size = winsize_for(GridSize {
            cols: 132,
            rows: 43,
            cell_width_px: 10,
            cell_height_px: 20,
        });
        assert_eq!(size.ws_col, 132);
        assert_eq!(size.ws_row, 43);
        assert_eq!(size.ws_xpixel, 1320);
        assert_eq!(size.ws_ypixel, 860);
    }
}
