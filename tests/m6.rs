//! M6 stress suite: deterministic headless workloads that print the counters
//! documented in docs/PERFORMANCE.md. Run with `--nocapture` to collect the
//! before/after numbers for a release measurement.

use std::sync::Arc;
use std::thread;

use pain::terminal::geometry::RowCache;
use pain::terminal::ghostty::ghostty_vt_sys as sys;
use pain::terminal::session::{CountersSnapshot, NoopWakeup, TerminalSession};

fn report(label: &str, counters: &CountersSnapshot) {
    println!(
        "M6 {label}: writes={} bytes={} wakeups_scheduled={} wakeups_coalesced={} \
         render_updates={} dirty_full={} dirty_partial={} dirty_clean={} \
         cursor_only_frames={} rows_rebuilt={} shape_line_calls={} \
         lock_max_us={} prepaint_us={}",
        counters.vt_write_calls,
        counters.vt_write_bytes,
        counters.wakeups_scheduled,
        counters.wakeups_coalesced,
        counters.render_state_updates,
        counters.dirty_full,
        counters.dirty_partial,
        counters.dirty_clean,
        counters.cursor_only_frames,
        counters.rows_rebuilt,
        counters.shape_line_calls,
        counters.terminal_lock_hold_max_micros,
        counters.prepaint_micros,
    );
}

#[test]
fn output_flood_coalesces_wakeups_and_bounds_render_work() {
    let session = Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
    let producers: Vec<_> = (0..4)
        .map(|thread_index| {
            let session = Arc::clone(&session);
            thread::spawn(move || {
                for index in 0..2000 {
                    let line = format!("t{thread_index}:{index:05} 0123456789abcdef\r\n");
                    session.write_vt(line.as_bytes());
                }
            })
        })
        .collect();

    let mut frames = 0usize;
    while producers.iter().any(|handle| !handle.is_finished()) {
        let snapshot = session.render_snapshot().expect("render during flood");
        assert_eq!(snapshot.cols, 80);
        frames += 1;
    }
    for handle in producers {
        handle.join().expect("producer joined");
    }
    let final_frame = session.render_snapshot().expect("final frame");
    assert!(!final_frame.lines[0].is_empty());

    let counters = session.snapshot_counters();
    report("output-flood", &counters);
    assert_eq!(counters.vt_write_calls, 8000);
    assert!(
        counters.wakeups_scheduled * 10 < counters.vt_write_calls,
        "a flood must not schedule one UI callback per chunk"
    );
    assert!(frames > 0);
    assert!(
        counters.terminal_lock_hold_max_micros < 1_000_000,
        "terminal lock hold must stay bounded to VT work"
    );
    assert!(counters.dirty_rows_visited > 0);
}

#[test]
fn cursor_only_frame_rebuilds_no_rows() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    session.write_vt(b"prompt $ ");
    session.render_snapshot().expect("first frame");
    session.mark_frame_complete().expect("first frame consumed");

    // Hide/show cursor is a mode change: the visible glyph rows stay clean.
    session.write_vt(b"\x1b[?25l");
    let snapshot = session.render_snapshot().expect("cursor frame");
    let mut cache = RowCache::new(usize::from(snapshot.rows));
    // Prime the cache with the preceding complete frame.
    let _ = cache.rows_to_rebuild(1, sys::GHOSTTY_RENDER_STATE_DIRTY_FULL, &[]);
    let rebuild = cache.rows_to_rebuild(
        2,
        snapshot.dirty,
        &snapshot.dirty_rows,
    );

    let counters = session.snapshot_counters();
    report("cursor-only", &counters);
    assert_eq!(snapshot.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    assert!(snapshot.dirty_rows.is_empty());
    assert!(rebuild.is_empty(), "cursor blink must not reshape visible rows");
    assert_eq!(counters.cursor_only_frames, 1);
}

#[test]
fn resize_storm_coalesces_to_distinct_grid_sizes() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    let mut last = (80u16, 24u16);
    for index in 0..1000 {
        // A drag emits many identical intermediate bounds and only occasionally
        // crosses a cell boundary. Alternating sizes every call would be 1000
        // legitimate grid changes, not a coalescing case.
        let step = index / 100;
        let size = (80 + step as u16 * 2, 24 + step as u16);
        let _ = session.resize(size.0, size.1, 9, 18);
        last = size;
    }
    let counters = session.snapshot_counters();
    report("resize-storm", &counters);
    assert_eq!(last, (98, 33));
    assert_eq!(counters.bounds_changes_observed, 1000);
    assert_eq!(counters.ghostty_resize_calls, 9);
    assert_eq!(counters.duplicate_resize_suppressed, 991);
}
