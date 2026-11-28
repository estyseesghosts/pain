//! M3/M4 headless PTY smoke tests. These exercise the real OS transport and
//! the shared session without opening a GPUI window, so they can run in CI and
//! in WSL. Each test launches a real shell, writes input, reads parsed output
//! through Ghostty, resizes the grid and shuts down cleanly.

use std::sync::Arc;
use std::time::{Duration, Instant};

use pain::platform::{spawn_pty, PtyConfig, PtyEventSink, ShellCommand};
use pain::terminal::session::{NoopWakeup, TerminalSession};

/// These gates exercise transport and parsing, not host presentation. They
/// supply a sink that deliberately discards events instead of mocking the
/// real PTY boundary.
fn no_events() -> PtyEventSink {
    Arc::new(|_| {})
}

#[cfg(target_os = "windows")]
fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[((n >> 18) & 63) as usize] as char);
        out.push(A[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            A[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(target_os = "windows")]
static NEXT_APC_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(target_os = "windows")]
fn wait_for_image(session: &TerminalSession, image_id: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let snapshot = session.render_snapshot().expect("render snapshot");
        let found = snapshot
            .image_placements
            .iter()
            .any(|placement| placement.image_id == image_id);
        session.mark_frame_complete().expect("frame complete");
        if found {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[cfg(target_os = "linux")]
fn wait_for_any_image(session: &TerminalSession) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let snapshot = session.render_snapshot().expect("render snapshot");
        let found = !snapshot.image_placements.is_empty();
        session.mark_frame_complete().expect("frame complete");
        if found {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

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
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand::new("/bin/sh"),
    };
    let pty = spawn_pty(&config, Arc::clone(&session), no_events()).expect("spawn /bin/sh");

    session
        .enqueue_writer(b"echo PAIN_LINUX_MARKER\n".to_vec())
        .expect("enqueue echo");
    assert!(
        wait_for(&session, "PAIN_LINUX_MARKER"),
        "shell output never reached Ghostty: {}",
        snapshot_text(&session)
    );

    // Distinct grid size reaches the PTY; a duplicate is coalesced by the
    // backend and must not disturb the session.
    pty.request_resize(100, 30, 10, 20).expect("resize PTY");
    pty.request_resize(100, 30, 10, 20)
        .expect("duplicate resize");
    session
        .enqueue_writer(b"stty size\n".to_vec())
        .expect("enqueue stty");
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
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand::new("C:\\Windows\\System32\\cmd.exe"),
    };
    let pty = spawn_pty(&config, Arc::clone(&session), no_events()).expect("spawn cmd.exe");

    // The ConPTY host (OpenConsole) starts asynchronously; wait for its first
    // output before sending input so the command is not lost during startup.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && snapshot_text(&session).is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    session
        .enqueue_writer(b"echo PAIN_WINDOWS_MARKER\r\n".to_vec())
        .expect("enqueue echo");
    assert!(
        wait_for(&session, "PAIN_WINDOWS_MARKER"),
        "shell output never reached Ghostty: {}",
        snapshot_text(&session)
    );

    pty.request_resize(100, 30, 10, 20).expect("resize ConPTY");
    pty.request_resize(100, 30, 10, 20)
        .expect("duplicate resize");

    pty.shutdown();
}

#[cfg(target_os = "windows")]
#[test]
fn windows_conpty_runs_powershell() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand {
            program: "powershell.exe".into(),
            args: vec![
                "-NoProfile".into(),
                "-Command".into(),
                "Write-Output PAIN_PS_MARKER".into(),
            ],
        },
    };
    let pty = spawn_pty(&config, Arc::clone(&session), no_events()).expect("spawn powershell.exe");
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
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand {
            program: "vim.exe".into(),
            args: vec!["-u".into(), "NONE".into()],
        },
    };
    let pty = match spawn_pty(&config, Arc::clone(&session), no_events()) {
        Ok(pty) => pty,
        Err(error) => {
            eprintln!("skipping vim smoke test: {error}");
            return;
        }
    };

    // Vim draws an alternate screen immediately; wait for the first frame.
    // Distinguish an unusable host tool from a Pain transport regression by
    // watching the child as well as the terminal. Hosted runners can ship a
    // `vim.exe` that exits without rendering (a GUI launcher or a build with
    // no matching terminfo): that is an environment skip. A child which is
    // still alive at the deadline but delivered no bytes is a transport
    // failure, so the gate must fail instead of silently skipping.
    let mut saw_output = false;
    let mut child_exited = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Check output before exit so a child that emits a frame and then
        // exits immediately is still classified as "output arrived".
        if !snapshot_text(&session).is_empty() {
            saw_output = true;
            break;
        }
        if pty.process_exited() {
            child_exited = true;
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if !saw_output {
        pty.shutdown();
        if child_exited {
            eprintln!("SKIP vim smoke test: vim exited without rendering a frame");
            return;
        }
        panic!(
            "vim produced no terminal output while the ConPTY child was still \
             running: Pain is not transporting PTY bytes"
        );
    }

    // A mouse TUI enables tracking through a terminal mode report, which
    // Ghostty owns. The session must observe it without any Rust-side table.
    // Capture the observed state before shutdown so every path tears the
    // child down even when the assertion fails.
    session
        .enqueue_writer(b":set mouse=a\r".to_vec())
        .expect("enqueue vim mouse mode");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !session.mouse_tracking_enabled().unwrap_or(false) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let mouse_enabled = session.mouse_tracking_enabled().unwrap_or(false);

    session
        .enqueue_writer(b":q!\r".to_vec())
        .expect("enqueue vim quit");
    pty.shutdown();
    assert!(mouse_enabled, "vim did not enable mouse reporting");
}

#[cfg(target_os = "windows")]
#[test]
fn bundled_conpty_is_packaged_next_to_the_test_binary() {
    // The build script copies the pinned NuGet pair beside the executable. If
    // this fails, the Windows graphics gate cannot be trusted: the in-box host
    // drops Kitty APCs.
    assert!(
        pain::platform::windows::conpty_is_bundled(),
        "windows/conpty/x64 was not copied beside the test binary"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn conpty_apc_survives_to_ghostty_graphics() {
    if !pain::platform::windows::conpty_is_bundled() {
        eprintln!(
            "SKIP conpty APC gate: no bundled conpty.dll; the kernel32 fallback is \
             documented as Kitty-graphics-degraded"
        );
        return;
    }
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session
        .apply_layout_geometry(80, 24, 10, 20)
        .expect("geometry");
    // f=32 is RGBA: four bytes per pixel, base64 "/wAA/w==".
    let payload = b64(&[255u8, 0, 0, 255]);
    let apc = format!("\x1b_Ga=T,f=32,s=1,v=1,i=77,C=1,q=2;{payload}\x1b\\\r\n");
    let path = std::env::temp_dir().join(format!(
        "pain-conpty-apc-{}-{}.bin",
        std::process::id(),
        NEXT_APC_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&path, apc.as_bytes()).expect("write APC payload");
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand {
            program: r"C:\Windows\System32\cmd.exe".into(),
            args: vec![
                "/c".into(),
                "type".into(),
                path.to_string_lossy().into_owned(),
            ],
        },
    };
    let pty = spawn_pty(&config, Arc::clone(&session), no_events()).expect("spawn cmd.exe");
    assert!(
        wait_for_image(&session, 77),
        "the bundled ConPTY did not forward the Kitty APC to libghostty-vt"
    );
    pty.shutdown();
    let _ = std::fs::remove_file(path);
}

#[cfg(target_os = "windows")]
#[test]
fn wsl_client_apc_survives_wsl_pty_and_conpty() {
    if !pain::platform::windows::conpty_is_bundled() {
        eprintln!("SKIP WSL APC gate: bundled ConPTY is absent");
        return;
    }
    // Hosted runners expose wsl.exe but often have no installed distro. Probe
    // with a plain child process first: when bash cannot start there is no
    // Linux client to exercise, so skip instead of reporting a transport
    // failure. The bundled-ConPTY APC gate above still covers this host.
    // Success status alone is not evidence: a wrapper that exits zero without
    // executing bash would make this gate pass while exercising nothing. The
    // marker must actually appear on stdout.
    let probe = std::process::Command::new("wsl.exe")
        .args(["-e", "bash", "-c", "printf PAIN_WSL_READY"])
        .output();
    let wsl_ready = probe
        .as_ref()
        .map(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).contains("PAIN_WSL_READY")
        })
        .unwrap_or(false);
    if !wsl_ready {
        eprintln!(
            "SKIP WSL APC gate: no usable WSL distro is installed (expected \
             PAIN_WSL_READY on stdout)"
        );
        return;
    }
    // Linux client -> WSL PTY -> ConPTY -> Pain -> libghostty-vt. This is the
    // path a WSL kitten/icat actually takes.
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session
        .apply_layout_geometry(80, 24, 10, 20)
        .expect("geometry");
    // bash printf turns \033 into ESC and \134 into a single backslash, so the
    // final String terminator is the two bytes ESC \.
    let script = "printf '\\033_Ga=T,f=32,s=1,v=1,i=78,C=1,q=2;/wAA/w==\\033\\134'";
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand {
            program: "wsl.exe".into(),
            args: vec!["-e".into(), "bash".into(), "-c".into(), script.into()],
        },
    };
    let pty = match spawn_pty(&config, Arc::clone(&session), no_events()) {
        Ok(pty) => pty,
        Err(error) => {
            eprintln!("SKIP WSL APC gate: {error}");
            return;
        }
    };
    assert!(
        wait_for_image(&session, 78),
        "an APC produced inside WSL did not survive to libghostty-vt"
    );
    pty.shutdown();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_real_kitty_client_image_reaches_ghostty() {
    // A real Kitty-protocol client (chafa's kitty output), not a hand-written
    // APC: client -> Linux PTY -> Pain -> libghostty-vt. Skip only when the
    // optional client is not installed.
    let available = std::process::Command::new("chafa")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !available {
        eprintln!("SKIP real Kitty client gate: chafa is not installed");
        return;
    }
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session
        .apply_layout_geometry(80, 24, 10, 20)
        .expect("geometry");
    let path = std::env::temp_dir().join(format!("pain-chafa-{}.png", std::process::id()));
    let image = image::ImageBuffer::from_pixel(16, 16, image::Rgba([220u8, 40, 40, 255]));
    image.save(&path).expect("write test PNG");
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        command: ShellCommand {
            program: "chafa".into(),
            args: vec![
                "--format".into(),
                "kitty".into(),
                path.to_string_lossy().into_owned(),
            ],
        },
    };
    let pty = match spawn_pty(&config, Arc::clone(&session), no_events()) {
        Ok(pty) => pty,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            eprintln!("SKIP real Kitty client gate: {error}");
            return;
        }
    };
    let placed = wait_for_any_image(&session);
    pty.shutdown();
    let _ = std::fs::remove_file(&path);
    assert!(
        placed,
        "chafa's Kitty output never produced a placement in Ghostty"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_kitten_unicode_placeholder_reaches_ghostty() {
    // A real Kitty client using the Unicode-placeholder path, not a
    // hand-written APC: kitten -> Linux PTY -> Pain -> libghostty-vt.
    let available = std::process::Command::new("kitten")
        .args(["icat", "--help"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !available {
        eprintln!("SKIP kitten placeholder gate: kitten is not installed");
        return;
    }
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    session.install_pty_writer().expect("writer");
    session
        .apply_layout_geometry(80, 24, 10, 20)
        .expect("geometry");
    let path = std::env::temp_dir().join(format!("pain-kitten-{}.png", std::process::id()));
    let image = image::ImageBuffer::from_pixel(32, 32, image::Rgba([30u8, 200, 90, 255]));
    image.save(&path).expect("write test PNG");
    let config = PtyConfig {
        cols: 80,
        rows: 24,
        cell_width_px: 10,
        cell_height_px: 20,
        command: ShellCommand {
            program: "kitten".into(),
            args: vec![
                "icat".into(),
                "--transfer-mode=stream".into(),
                "--detect-support=no".into(),
                "--unicode-placeholder".into(),
                path.to_string_lossy().into_owned(),
            ],
        },
    };
    let pty = match spawn_pty(&config, Arc::clone(&session), no_events()) {
        Ok(pty) => pty,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            eprintln!("SKIP kitten placeholder gate: {error}");
            return;
        }
    };
    let placed = wait_for_any_image(&session);
    let snapshot = session.render_snapshot().expect("render snapshot");
    let virtual_placement = snapshot
        .image_placements
        .iter()
        .any(|placement| {
            placement.kind
                == pain::terminal::ghostty::ghostty_vt_sys::GHOSTTY_RENDER_STATE_KITTY_PLACEMENT_KIND_VIRTUAL
        });
    session.mark_frame_complete().expect("frame complete");
    pty.shutdown();
    let _ = std::fs::remove_file(&path);
    assert!(
        placed,
        "kitten's Unicode-placeholder image did not produce a placement"
    );
    assert!(
        virtual_placement,
        "kitten's U=1 transmission did not produce a virtual placement"
    );
}

/// The session's writer statistics include bytes produced by Ghostty effects
/// as well as UI input, and are reported through the common counter snapshot.
#[test]
fn writer_counters_are_reported() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    session
        .enqueue_writer(b"abc".to_vec())
        .expect("enqueue counter bytes");
    let counters = session.snapshot_counters();
    assert_eq!(counters.writer_messages, 1);
    assert_eq!(counters.writer_bytes_queued, 3);
    assert!(counters.writer_queue_depth_max >= 1);
}
