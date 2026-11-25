//! M0 acceptance: create, feed and inspect a Ghostty terminal without a
//! window, on Windows and Linux alike. Every Ghostty handle is dropped by the
//! wrapper; the repeated-create test exercises that path.

use pain::terminal::ghostty::{
    self, ghostty_vt_sys as sys, paste_encode, paste_is_safe, ColorSpec, GhosttyError,
    GhosttyRenderState, GhosttyTerminal, KeyEncoder, KeyEvent, RenderSnapshot,
};
use pain::terminal::session::{NoopWakeup, TerminalSession};

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

fn line_text(snapshot: &RenderSnapshot, y: usize) -> String {
    snapshot.lines[y]
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
    assert_eq!(snapshot.rows, 24);
    assert_ne!(
        snapshot.dirty,
        ghostty_vt_sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE
    );
    assert_eq!(line_text(&snapshot, 0), "Hi Green");
    assert_eq!(line_text(&snapshot, 1), "Bye");

    let green = &snapshot.lines[0][3];
    assert_eq!(green.text, "G");
    assert!(green.has_styling);
    assert_eq!(green.style.foreground, Some(ColorSpec::Palette(2)));
    assert!(green.style.is_bold());
    assert_eq!(snapshot.lines[0][2].text, " ");
    assert!(snapshot.lines[0][2].style.foreground.is_none());

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

    let cell = &snapshot.lines[0][0];
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
    assert_eq!((snapshot.cols, snapshot.rows), (40, 10));
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
