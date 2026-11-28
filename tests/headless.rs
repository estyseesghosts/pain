//! M0 acceptance: create, feed and inspect a Ghostty terminal without a
//! window, on Windows and Linux alike. Every Ghostty handle is dropped by the
//! wrapper; the repeated-create test exercises that path.

use pain::terminal::ghostty::{
    self, ghostty_vt_sys as sys, paste_encode, paste_is_safe, ColorSpec, GhosttyError,
    GhosttyRenderState, GhosttyTerminal, KeyEncoder, KeyEvent, RenderSnapshot,
};
use pain::terminal::session::{NoopWakeup, TerminalSession};

/// Space is a printable key whose GPUI name is multi-character (`"space"`).
/// Ghostty's encoder emits nothing for it unless UTF-8 text is supplied. This
/// documents why the view delegates plain printable keys to the platform text
/// path instead of encoding them: a bare `GHOSTTY_KEY_SPACE` writes zero bytes.
#[test]
fn printable_named_keys_require_utf8_text() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");

    let mut bare = KeyEvent::new().expect("key event");
    bare.press(sys::GHOSTTY_KEY_SPACE, 0).expect("press space");
    let without_text = session.encode_key(&bare).expect("encode bare space");
    assert_eq!(
        without_text, b"",
        "Ghostty emits no bytes for space without text"
    );

    let mut with_text = KeyEvent::new().expect("key event");
    with_text
        .press(sys::GHOSTTY_KEY_SPACE, 0)
        .expect("press space");
    with_text.set_utf8(" ").expect("utf8");
    let bytes = session
        .encode_key(&with_text)
        .expect("encode space with text");
    assert_eq!(bytes, b" ", "space with UTF-8 must reach the PTY");
}

/// Named keys (which never carry IME text) must still reach the PTY as the
/// control bytes a shell expects. This is the encoder half of the
/// Enter/Backspace input path used by `TerminalView::on_key_down`.
#[test]
fn named_control_keys_encode_to_pty_bytes() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");

    let mut enter = KeyEvent::new().expect("key event");
    enter.press(sys::GHOSTTY_KEY_ENTER, 0).expect("press enter");
    let bytes = session.encode_key(&enter).expect("encode enter");
    assert!(
        bytes.contains(&b'\r') || bytes.contains(&b'\n'),
        "enter encoded to {bytes:?}"
    );

    let mut backspace = KeyEvent::new().expect("key event");
    backspace
        .press(sys::GHOSTTY_KEY_BACKSPACE, 0)
        .expect("press backspace");
    let bytes = session.encode_key(&backspace).expect("encode backspace");
    assert!(
        bytes.contains(&0x7f) || bytes.contains(&0x08),
        "backspace encoded to {bytes:?}"
    );

    let mut ctrl_c = KeyEvent::new().expect("key event");
    ctrl_c.set_utf8("c").expect("utf8");
    ctrl_c
        .press(sys::GHOSTTY_KEY_C, sys::GHOSTTY_MODS_CTRL)
        .expect("press ctrl-c");
    let bytes = session.encode_key(&ctrl_c).expect("encode ctrl-c");
    assert_eq!(bytes, b"\x03");
}

/// End-to-end check of the ownership split: plain text goes straight to the
/// writer queue, while modified keys are encoded by Ghostty into terminal
/// sequences. This is the same path `TerminalView::on_key_down` drives.
#[test]
fn keyboard_policy_reaches_the_pty_correctly() {
    use pain::terminal::input::{key_for_keystroke, Modifiers};

    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");

    // The space bar is plain text: the platform text handler owns it.
    assert!(
        key_for_keystroke("space", Some(" "), Modifiers::default(), false, false).is_none(),
        "space must not be encoded twice"
    );
    session.write_input(" ").expect("commit text");
    assert_eq!(session.drain_writer(8), vec![b" ".to_vec()]);

    // Ctrl+C is a control key: Ghostty encodes it to the C0 byte.
    let encoded = key_for_keystroke(
        "c",
        None,
        Modifiers::new(false, true, false, false),
        false,
        false,
    )
    .expect("ctrl-c is encoded");
    let mut key = KeyEvent::new().expect("key event");
    key.press(encoded.key, encoded.mods.ghostty_bits())
        .expect("press");
    if let Some(text) = &encoded.text {
        key.set_utf8(text).expect("utf8");
    }
    key.set_unshifted_codepoint(encoded.unshifted_codepoint)
        .expect("unshifted codepoint");

    let bytes = session.encode_key(&key).expect("encode ctrl-c");
    assert_eq!(bytes, b"\x03");
    assert_eq!(session.drain_writer(8), vec![vec![0x03]]);
}

/// Palette choices must reach Ghostty's render state, not just the settings
/// document. This covers both a named palette and the light/dark Theme palette.
#[test]
fn selected_palette_reaches_ghostty_render_state() {
    use pain::terminal::palette::{resolve, PaletteChoice};

    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    let mocha = resolve(PaletteChoice::CatppuccinMocha, true).expect("mocha palette");
    session.set_colors(Some(&mocha)).expect("apply mocha");
    let colors = session.render_snapshot().expect("snapshot").colors.clone();
    assert_eq!(colors.background, (0x1e, 0x1e, 0x2e));
    assert_eq!(colors.palette[1], (0xf3, 0x8b, 0xa8));

    let light = resolve(PaletteChoice::Theme, false).expect("light theme palette");
    session
        .set_colors(Some(&light))
        .expect("apply light palette");
    let colors = session.render_snapshot().expect("snapshot").colors.clone();
    assert_eq!(colors.background, (0xff, 0xff, 0xff));
    assert_eq!(colors.foreground, (0x1a, 0x1a, 0x1a));

    // Reset restores Ghostty's built-in palette without leaking the previous
    // default colours into the render state.
    session.set_colors(None).expect("reset palette");
    let reset = session.render_snapshot().expect("snapshot").colors.clone();
    assert_eq!(reset.background, (0, 0, 0));
    assert_eq!(reset.foreground, (0xff, 0xff, 0xff));
}

/// Mouse reporting must be decided by Ghostty's current modes, not by a Rust
/// mode table in the view. The view now sends every motion (with `None` when no
/// button is held); the encoder drops it in modes 1000/1002 and reports it in
/// 1003. This guards the "mouse reporting feels off" regression.
#[test]
fn mouse_motion_follows_terminal_tracking_modes() {
    use pain::terminal::input::{MouseAction, MouseButton};

    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    let encode = |session: &TerminalSession, button, held, x, y| {
        session
            .encode_mouse(
                MouseAction::Motion.ghostty(),
                button,
                0,
                x,
                y,
                held,
                800,
                600,
                10,
                25,
            )
            .expect("encode motion")
    };

    // No tracking: nothing is ever reported.
    assert!(encode(&session, None, false, 100.0, 100.0).is_empty());

    // 1003 any-event + 1006 SGR: hover motion with no button is a real report.
    session.write_vt(b"\x1b[?1003h\x1b[?1006h");
    let hover = encode(&session, None, false, 100.0, 100.0);
    assert!(
        hover.starts_with(b"\x1b[<35;"),
        "any-motion should be SGR button 35, got {hover:?}"
    );
    assert!(hover.ends_with(b"M"));

    // A drag that leaves the surface is only reported when a button is held.
    let outside = encode(&session, None, true, -5.0, 100.0);
    assert!(
        !outside.is_empty(),
        "held drag outside viewport must report"
    );
    let outside_no_button = encode(&session, None, false, -5.0, 200.0);
    assert!(
        outside_no_button.is_empty(),
        "unheld motion outside viewport must be dropped"
    );

    // 1002 button-event: hover is dropped, a held-button drag is reported.
    session.write_vt(b"\x1b[?1003l\x1b[?1002h");
    assert!(encode(&session, None, false, 300.0, 300.0).is_empty());
    let drag = encode(&session, MouseButton::Left.ghostty(), true, 300.0, 300.0);
    assert!(
        drag.starts_with(b"\x1b[<32;"),
        "left drag should be SGR button 32, got {drag:?}"
    );
}

/// Clipboard paste must use Ghostty's bracketed-paste policy, never a raw
/// write. The action path calls exactly this method on both platforms.
#[test]
fn clipboard_paste_uses_bracketed_mode() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    let plain = session.paste("hello").expect("plain paste");
    assert_eq!(plain, b"hello");

    session.write_vt(b"\x1b[?2004h");
    let bracketed = session.paste("hello").expect("bracketed paste");
    assert_eq!(bracketed, b"\x1b[200~hello\x1b[201~");
    assert_eq!(
        session.drain_writer(8),
        vec![b"hello".to_vec(), bracketed.clone()]
    );
}

fn line_text(snapshot: &RenderSnapshot, y: usize) -> String {
    snapshot
        .line(y)
        .iter()
        .map(|cell| cell.text.as_str())
        .collect()
}

#[test]
fn create_feed_render_and_inspect_styled_cells() {
    let mut terminal = GhosttyTerminal::new(80, 24).expect("terminal created");
    assert_eq!(terminal.cols(), 80);
    assert_eq!(terminal.rows(), 24);

    terminal.write(b"Hi \x1b[1;32mGreen\x1b[0m\r\nBye");

    let mut state = GhosttyRenderState::new().expect("render state created");
    state.update(&terminal).expect("render state updated");
    let snapshot = state.snapshot().expect("snapshot read");

    assert_eq!(snapshot.cols, 80);
    assert_eq!(snapshot.meta.rows, 24);
    assert_ne!(
        snapshot.dirty,
        ghostty_vt_sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE
    );
    assert_eq!(line_text(&snapshot, 0), "Hi Green");
    assert_eq!(line_text(&snapshot, 1), "Bye");

    let green = &snapshot.line(0)[3];
    assert_eq!(green.text, "G");
    assert!(green.has_styling);
    assert_eq!(green.style.foreground, Some(ColorSpec::Palette(2)));
    assert!(green.style.is_bold());
    assert_eq!(snapshot.line(0)[2].text, " ");
    assert!(snapshot.line(0)[2].style.foreground.is_none());

    assert!(snapshot.cursor.has_value);
    assert!(snapshot.cursor.visible);
    assert_eq!((snapshot.cursor.x, snapshot.cursor.y), (3, 1));

    // A frame's dirty flags are only cleared after every row was consumed.
    state.clean().expect("render state cleaned");
    let clean = state.snapshot().expect("clean snapshot read");
    assert_eq!(
        clean.dirty,
        ghostty_vt_sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE
    );
    assert_eq!(line_text(&clean, 0), "Hi Green");
}

#[test]
fn true_color_and_bold_survive_the_render_bridge() {
    let mut terminal = GhosttyTerminal::new(20, 4).expect("terminal created");
    terminal.write(b"\x1b[1;38;2;255;16;32mR\x1b[0m");

    let mut state = GhosttyRenderState::new().expect("render state created");
    state.update(&terminal).expect("render state updated");
    let snapshot = state.snapshot().expect("snapshot read");

    let cell = &snapshot.line(0)[0];
    assert_eq!(cell.text, "R");
    assert!(cell.style.is_bold());
    assert_eq!(cell.style.foreground, Some(ColorSpec::Rgb(255, 16, 32)));
    assert_eq!(cell.foreground, Some((255, 16, 32)));
}

#[test]
fn resize_updates_grid_dimensions() {
    let mut terminal = GhosttyTerminal::new(80, 24).expect("terminal created");
    terminal
        .resize(40, 10, 8, 16)
        .expect("terminal resized with cell pixels");
    assert_eq!(
        (
            terminal
                .get_u16(ghostty_vt_sys::GHOSTTY_TERMINAL_DATA_COLS)
                .unwrap(),
            terminal
                .get_u16(ghostty_vt_sys::GHOSTTY_TERMINAL_DATA_ROWS)
                .unwrap(),
        ),
        (40, 10)
    );

    let mut state = GhosttyRenderState::new().unwrap();
    state.update(&terminal).unwrap();
    let snapshot = state.snapshot().unwrap();
    assert_eq!((snapshot.cols, snapshot.meta.rows), (40, 10));
}

#[test]
fn key_encoder_syncs_and_encodes_without_a_window() {
    let terminal = GhosttyTerminal::new(80, 24).expect("terminal created");
    let mut encoder = KeyEncoder::new().expect("encoder created");
    encoder
        .sync_from_terminal(&terminal)
        .expect("encoder synced from terminal modes");

    let mut event = KeyEvent::new().expect("key event created");
    event
        .press(ghostty_vt_sys::GHOSTTY_KEY_A, 0)
        .expect("key event configured");
    event.set_utf8("a").expect("key text set");

    let mut buf = [0u8; 128];
    let written = encoder.encode(&event, &mut buf).expect("key encoded");
    assert!(written > 0, "encoder produced no bytes");
    assert_eq!(&buf[..written], b"a");
}

#[test]
fn paste_helpers_follow_ghostty_policy() {
    assert!(paste_is_safe("hello world"));
    assert!(!paste_is_safe("hello\nworld"));
    assert!(!paste_is_safe("bad\x1b[201~"));

    let plain = paste_encode("hello", false).expect("plain paste encoded");
    assert_eq!(plain, b"hello");

    let bracketed = paste_encode("hello", true).expect("bracketed paste encoded");
    assert!(bracketed.starts_with(b"\x1b[200~"));
    assert!(bracketed.ends_with(b"\x1b[201~"));
}

#[test]
fn repeated_create_free_loop_keeps_every_handle_accounted_for() {
    const ITERATIONS: usize = 256;
    let linked = ghostty::version();
    assert!(!linked.is_empty());

    for _ in 0..ITERATIONS {
        let mut terminal = GhosttyTerminal::new(80, 24).expect("terminal created");
        terminal.write(b"loop\r\n");
        let mut state = GhosttyRenderState::new().expect("render state created");
        state.update(&terminal).expect("render state updated");
        let snapshot = state.snapshot().expect("snapshot read");
        assert_eq!(line_text(&snapshot, 0), "loop");
        // All four handles (terminal, state, row iterator, cells) are dropped
        // at scope end through their `Drop` implementations.
    }
}

#[test]
fn error_results_stay_as_typed_errors() {
    // A zero-sized terminal is rejected by the library rather than panicking.
    let error: Result<GhosttyTerminal, GhosttyError> = GhosttyTerminal::new(0, 0);
    assert!(error.is_err());
}
