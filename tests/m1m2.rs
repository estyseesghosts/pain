//! M1/M2 acceptance through the public API, without opening a window.
//!
//! M1's rendering decisions are pure functions in `pain_terminal_core`; M2's
//! session/concurrency model is entirely headless. Together these tests cover
//! the acceptance gates that do not require launching the GPUI window.

use std::sync::Arc;

use pain::fixtures::Fixture;
use pain_terminal_core::{
    adjust_font_size, batch_row, display_columns, selection_span, CellMetrics, NoopWakeup,
    RowCache, TerminalSession, FONT_SIZE_STEP, MAX_FONT_SIZE, MIN_FONT_SIZE,
};

#[test]
fn m1_canned_fixtures_render_styled_wide_and_selected_cells() {
    for fixture in Fixture::ALL {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        fixture.apply_session(&session);
        let snapshot = session.render_snapshot().expect("render snapshot");
        assert_eq!((snapshot.cols, snapshot.meta.rows), (80, 24));

        match fixture {
            Fixture::Ansi => {
                assert_eq!(snapshot.line(0)[0].text, "H");
                assert!(snapshot.line(0)[3].style.is_bold());
                assert!(snapshot.cursor.has_value);
            }
            Fixture::Colors => {
                assert!(snapshot.line(0)[0].style.foreground.is_some());
                assert!(snapshot.line(0)[4].style.foreground.is_some());
            }
            Fixture::WideAndStyles => {
                assert_eq!(display_columns("界"), 2);
                assert!(snapshot.line(1).iter().any(|c| c.text == "界"));
                assert!(snapshot.line(0).iter().any(|c| c.style.underline() != 0));
            }
            Fixture::Selection => {
                assert_eq!(
                    snapshot.selections[0].map(|s| (s.start_x, s.end_x)),
                    Some((0, 7))
                );
                assert!(snapshot.line(0)[0].selected);
                assert!(!snapshot.line(0)[8].selected);
                assert_eq!(selection_span(snapshot.selections[0]), Some((0, 8)));
            }
        }

        session.mark_frame_complete().expect("frame consumed");
    }
}

#[test]
fn m1_unchanged_frame_does_not_rebuild_rows() {
    let mut cache = RowCache::new(24);
    assert_eq!(cache.rows_to_rebuild(1, 2, &[]).len(), 24);
    assert!(cache.rows_to_rebuild(1, 0, &[]).is_empty());
    assert_eq!(cache.rows_to_rebuild(2, 1, &[5, 6]).len(), 2);
    assert!(cache.rows_to_rebuild(3, 0, &[]).is_empty());
}

#[test]
fn m1_batching_and_cell_geometry_use_real_metrics() {
    let metrics = CellMetrics::new(9.6, 19.2);
    let (cols, rows) = metrics.grid_for(192.0, 384.0);
    assert_eq!((cols, rows), (20, 20));
    let (x, y, w, h) = metrics.span_bounds((0.0, 0.0), 2, 3, 4);
    assert!((x - 19.2).abs() < 0.001);
    assert!((y - 57.6).abs() < 0.001);
    assert!((w - 38.4).abs() < 0.001);
    assert!((h - 19.2).abs() < 0.001);

    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    Fixture::Ansi.apply_session(&session);
    let snapshot = session.render_snapshot().expect("snapshot");
    let runs = batch_row(snapshot.line(0));
    assert!(!runs.is_empty());
    assert!(runs.iter().any(|run| run.style.bold));
    assert!(runs.iter().all(|run| !run.text.is_empty()));
}

#[test]
fn font_size_steps_change_cell_metrics_and_clamp() {
    let base = 12.0;
    let bigger = adjust_font_size(base, FONT_SIZE_STEP);
    let smaller = adjust_font_size(base, -FONT_SIZE_STEP);
    assert!(bigger > base && smaller < base);
    // Larger font means fewer columns/rows in the same content bounds.
    let small = CellMetrics::new(9.0, 18.0);
    let large = CellMetrics::new(11.0, 22.0);
    assert!(small.grid_for(880.0, 440.0).0 > large.grid_for(880.0, 440.0).0);
    assert_eq!(adjust_font_size(MIN_FONT_SIZE, -100.0), MIN_FONT_SIZE);
    assert_eq!(adjust_font_size(MAX_FONT_SIZE, 100.0), MAX_FONT_SIZE);
}

#[test]
fn pixel_only_window_resize_does_not_resize_pty_grid() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    let first = session
        .apply_layout_geometry(80, 24, 9, 18)
        .expect("cell pixel geometry");
    assert!(first.terminal_changed);
    assert!(!first.grid_changed);
    let duplicate = session
        .apply_layout_geometry(80, 24, 9, 18)
        .expect("duplicate suppressed");
    assert!(!duplicate.changed());
    let grid = session
        .apply_layout_geometry(79, 24, 10, 19)
        .expect("grid changed");
    assert!(grid.terminal_changed && grid.grid_changed);
    let counters = session.snapshot_counters();
    assert_eq!(counters.ghostty_geometry_updates, 2);
    assert_eq!(counters.pty_grid_resize_requests, 1);
    assert_eq!(counters.duplicate_resize_suppressed, 1);
}

#[test]
fn m2_synthetic_producer_coalesces_and_session_is_shareable() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    let producer = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || {
            for i in 0..1000 {
                session.write_vt(format!("line {i}\r\n").as_bytes());
            }
        })
    };
    let mut frames = 0;
    while !producer.is_finished() {
        let _ = session.render_snapshot().expect("render during output");
        frames += 1;
    }
    producer.join().expect("producer finished");
    let counters = session.snapshot_counters();
    assert_eq!(counters.vt_write_calls, 1000);
    assert_eq!(counters.wakeups_scheduled, 1);
    assert_eq!(counters.wakeups_coalesced, 999);
    assert!(session.take_wakeup().is_some());
    assert!(frames > 0);
    assert!(counters.terminal_lock_hold_max_micros < 1_000_000);
}
