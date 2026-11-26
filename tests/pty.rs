//! M3/M4 headless PTY smoke tests. These exercise the real OS transport and
//! the shared session without opening a GPUI window, so they can run in CI and
//! in WSL. Each test launches a real shell, writes input, reads parsed output
//! through Ghostty, resizes the grid and shuts down cleanly.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pain::platform::{spawn_pty, PtyConfig, ShellCommand};
use pain::terminal::session::{NoopWakeup, TerminalSession};

fn snapshot_text(session: &TerminalSession) -> String {
    let snapshot = session.render_snapshot().expect("render snapshot");
    session.mark_frame_complete().expect("frame complete");
    snapshot
        .rows
        .iter()
        .flat_map(|(_, row)| row.iter())
        .map(|cell| cell.text.as_str())
        .collect::<String>()
}

fn wait_for(session: &TerminalSession, marker: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if snapshot_text(session).contains(marker) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(target_os = "linux")]
#[test]
fn linux_pty_runs_shell_input_resize_and_shutdown() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session.install_pty_writer().expect("write bridge");
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        command: ShellCommand::new("/bin/sh"),
    };
    let pty = spawn_pty(&config, Arc::clone(&session)).expect("spawn /bin/sh");

    session.enqueue_writer(b"echo PAIN_LINUX_MARKER\n".to_vec());
    assert!(
        wait_for(&session, "PAIN_LINUX_MARKER"),
        "shell output never reached Ghostty: {}",
        snapshot_text(&session)
    );

    // Distinct grid size reaches the PTY; a duplicate is coalesced by the
    // backend and must not disturb the session.
    pty.resize(100, 30).expect("resize PTY");
    pty.resize(100, 30).expect("duplicate resize");
    session.enqueue_writer(b"stty size\n".to_vec());
    assert!(
        wait_for(&session, "30 100"),
        "stty size did not reflect resize: {}",
        snapshot_text(&session)
    );

    pty.shutdown();
    assert!(
        session.process_exited(),
        "child exit was not observed and reaped"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_conpty_runs_cmd_input_resize_and_shutdown() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session.install_pty_writer().expect("write bridge");
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        command: ShellCommand::new("C:\\Windows\\System32\\cmd.exe"),
    };
    let pty = spawn_pty(&config, Arc::clone(&session)).expect("spawn cmd.exe");

    // The ConPTY host (OpenConsole) starts asynchronously; wait for its first
    // output before sending input so the command is not lost during startup.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && snapshot_text(&session).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    session.enqueue_writer(b"echo PAIN_WINDOWS_MARKER\r\n".to_vec());
    assert!(
        wait_for(&session, "PAIN_WINDOWS_MARKER"),
        "shell output never reached Ghostty: {}",
        snapshot_text(&session)
    );

    pty.resize(100, 30).expect("resize ConPTY");
    pty.resize(100, 30).expect("duplicate resize");

    pty.shutdown();
}

#[cfg(target_os = "windows")]
#[test]
fn windows_conpty_runs_powershell() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        command: ShellCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoProfile".into(),
                "-Command".into(),
                "Write-Output PAIN_PS_MARKER".into(),
            ],
        },
    };
    let pty = spawn_pty(&config, Arc::clone(&session)).expect("spawn powershell.exe");
    assert!(
        wait_for(&session, "PAIN_PS_MARKER"),
        "PowerShell output never reached Ghostty: {}",
        snapshot_text(&session)
    );
    pty.shutdown();
}

#[cfg(target_os = "windows")]
#[test]
fn windows_conpty_runs_vim_alternate_screen_and_mouse_mode() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        command: ShellCommand {
            program: "vim.exe".into(),
            args: vec!["-u".into(), "NONE".into()],
        },
    };
    let pty = match spawn_pty(&config, Arc::clone(&session)) {
        Ok(pty) => pty,
        Err(error) => {
            eprintln!("skipping vim smoke test: {error}");
            return;
        }
    };

    // Vim draws an alternate screen immediately; wait for the first frame.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && snapshot_text(&session).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !snapshot_text(&session).is_empty(),
        "vim produced no output"
    );

    // A mouse TUI enables tracking through a terminal mode report, which
    // Ghostty owns. The session must observe it without any Rust-side table.
    session.enqueue_writer(b":set mouse=a\r".to_vec());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !session.mouse_tracking_enabled().unwrap_or(false) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        session.mouse_tracking_enabled().unwrap_or(false),
        "vim did not enable mouse reporting"
    );

    session.enqueue_writer(b":q!\r".to_vec());
    pty.shutdown();
}

/// The session's writer statistics include bytes produced by Ghostty effects
/// as well as UI input, and are reported through the common counter snapshot.
#[test]
fn writer_counters_are_reported() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    session.enqueue_writer(b"abc".to_vec());
    let counters = session.snapshot_counters();
    assert_eq!(counters.writer_messages, 1);
    assert_eq!(counters.writer_bytes_queued, 3);
    assert!(counters.writer_queue_depth_max >= 1);
}
