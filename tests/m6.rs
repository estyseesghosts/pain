//! M6 stress suite: deterministic headless workloads that print the counters
//! documented in docs/PERFORMANCE.md. Run with `--nocapture` to collect the
//! before/after numbers for a release measurement.

use std::sync::Arc;
use std::thread;

use pain::terminal::geometry::RowCache;
use pain::terminal::ghostty::ghostty_vt_sys as sys;
use pain::terminal::session::{CountersSnapshot, NoopWakeup, TerminalSession};fn report(label: &str, counters: &CountersSnapshot) {
    println!(
        "M6 {label}: writes={} bytes={} wakeups_scheduled={} wakeups_coalesced={} \
         render_updates={} dirty_full={} dirty_partial={} dirty_clean={} \
         cursor_only_frames={} cells_materialized={} rows_materialized={} \
         rows_rebuilt={} row_cache_copies={} shape_line_calls={} \
         mouse_offered={} mouse_encoded={} mouse_bytes={} \
         ghostty_geometry_updates={} pty_grid_resize_requests={} \
         scroll_locks={} lock_max_us={} prepaint_us={} \
         img_rebuilds={} img_cache_hits={} img_pixel_reads={} img_pixel_bytes={} \
         img_races={} img_uploads={} img_evictions={} img_copy_lock_max_us={} \
         anim_ticks={} anim_frames={}",
        counters.vt_write_calls,
        counters.vt_write_bytes,
        counters.wakeups_scheduled,
        counters.wakeups_coalesced,
        counters.render_state_updates,
        counters.dirty_full,
        counters.dirty_partial,
        counters.dirty_clean,
        counters.cursor_only_frames,
        counters.cells_materialized,
        counters.rows_materialized,
        counters.rows_rebuilt,
        counters.row_cache_copies,
        counters.shape_line_calls,
        counters.mouse_events_offered,
        counters.mouse_events_encoded,
        counters.mouse_bytes_queued,
        counters.ghostty_geometry_updates,
        counters.pty_grid_resize_requests,
        counters.scroll_lock_acquisitions,
        counters.terminal_lock_hold_max_micros,
        counters.prepaint_micros,
        counters.image_placement_list_rebuilds,
        counters.image_placement_cache_hits,
        counters.image_pixel_reads,
        counters.image_pixel_bytes_copied,
        counters.image_generation_races,
        counters.image_texture_uploads,
        counters.image_texture_evictions,
        counters.image_copy_lock_max_micros,
        counters.kitty_animation_ticks,
        counters.kitty_animation_frames,
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
    assert!(!final_frame.line(0).is_empty());

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
    assert!(counters.rows_materialized > 0);
    assert!(counters.cells_materialized > 0);
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
    let mut cache = RowCache::new(usize::from(snapshot.meta.rows));
    // Prime the cache with the preceding complete frame.
    let _ = cache.rows_to_rebuild(1, sys::GHOSTTY_RENDER_STATE_DIRTY_FULL, &[]);
    let rebuild = cache.rows_to_rebuild(2, snapshot.dirty, &snapshot.dirty_rows);

    let counters = session.snapshot_counters();
    report("cursor-only", &counters);
    assert_eq!(snapshot.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    assert!(snapshot.dirty_rows.is_empty());
    assert!(
        rebuild.is_empty(),
        "cursor blink must not reshape visible rows"
    );
    assert_eq!(counters.cursor_only_frames, 1);
    // No rows/cells may be materialized for the cursor-only frame.
    assert_eq!(counters.rows_materialized, 24);
    assert_eq!(counters.cells_materialized, 24 * 80);
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
        let _ = session.apply_layout_geometry(size.0, size.1, 9, 18);
        last = size;
    }
    let counters = session.snapshot_counters();
    report("resize-storm", &counters);
    assert_eq!(last, (98, 33));
    assert_eq!(counters.bounds_changes_observed, 1000);
    assert_eq!(counters.ghostty_geometry_updates, 1 + 9);
    assert_eq!(counters.pty_grid_resize_requests, 9);
    // Every call is either a real Ghostty geometry change or a suppressed
    // duplicate. The 10 changes are: 1 cell-pixel-only change on the first
    // call plus 9 grid changes as the drag crosses cell boundaries.
    assert_eq!(
        counters.duplicate_resize_suppressed,
        counters.bounds_changes_observed - counters.ghostty_geometry_updates
    );
    assert_eq!(counters.duplicate_resize_suppressed, 990);
}

fn b64(data: &[u8]) -> String {
    const A: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
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

fn place_static_image(session: &TerminalSession, id: u32) {
    let pixels = [0x11u8, 0x22, 0x33, 0xff];
    let apc = format!(
        "\x1b_Ga=T,f=32,s=1,v=1,i={id},C=1,q=2;{}\x1b\\",
        b64(&pixels)
    );
    session.write_vt(apc.as_bytes());
    let _ = session.drain_writer(16);
}

#[test]
fn static_image_plus_cursor_change_does_zero_image_work() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    session.apply_layout_geometry(80, 24, 9, 18).unwrap();
    place_static_image(&session, 1);
    let first = session.render_snapshot().expect("first frame");
    assert_eq!(first.image_placements.len(), 1);
    session.mark_frame_complete().expect("first complete");
    let after_first = session.snapshot_counters();
    assert_eq!(after_first.image_placement_list_rebuilds, 1);
    assert_eq!(after_first.image_pixel_reads, 0);

    // Cursor show/hide is a mode change. Images have their own generation, so
    // the placement list must be reused without any pixel reads.
    session.write_vt(b"\x1b[?25l");
    let second = session.render_snapshot().expect("cursor frame");
    session.mark_frame_complete().expect("cursor complete");
    let counters = session.snapshot_counters();
    report("static-image-cursor", &counters);
    assert_eq!(second.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    assert!(second.dirty_rows.is_empty());
    assert_eq!(second.image_placements.len(), 1);
    assert_eq!(
        counters.image_placement_list_rebuilds,
        after_first.image_placement_list_rebuilds,
        "an unchanged placement generation must not rebuild the list"
    );
    assert!(counters.image_placement_cache_hits >= 1);
    assert_eq!(counters.image_pixel_reads, 0);
    assert_eq!(counters.image_pixel_bytes_copied, 0);
}

#[test]
fn scrolling_rebuilds_placements_but_not_pixels() {
    let session = TerminalSession::new(20, 10, Box::new(NoopWakeup)).expect("session");
    session.apply_layout_geometry(20, 10, 9, 18).unwrap();
    place_static_image(&session, 2);
    let first = session.render_snapshot().expect("frame");
    let p = first.image_placements[0].clone();
    session.mark_frame_complete().expect("complete");
    let pixels = session.read_kitty_images(&[(2, p.image_generation)]);
    assert_eq!(pixels.len(), 1);
    let before = session.snapshot_counters();

    // Scroll the image out of its original row. The placement list may change
    // (positions move), but the image generation and its pixels do not.
    for _ in 0..8 {
        session.write_vt(b"line\r\n");
    }
    session.scroll(-1).expect("scroll");
    let moved = session.render_snapshot().expect("moved frame");
    session.mark_frame_complete().expect("complete");
    let moved_placement = moved.image_placements.iter().find(|q| q.image_id == 2);
    if let Some(moved_placement) = moved_placement {
        assert_eq!(moved_placement.image_generation, p.image_generation);
        let again = session.read_kitty_images(&[(2, p.image_generation)]);
        assert_eq!(again.len(), 1, "unchanged generation must remain readable");
    }
    let counters = session.snapshot_counters();
    report("placement-scroll", &counters);
    // A placement-list rebuild is allowed when positions move; what matters is
    // that the image generation did not change and no pixel re-read was wasted.
    assert!(counters.image_placement_list_rebuilds >= before.image_placement_list_rebuilds);
    assert_eq!(
        counters.image_pixel_reads,
        before.image_pixel_reads + 1,
        "only the explicit same-generation read above may copy pixels"
    );
    assert_eq!(counters.image_generation_races, 0);
}

#[test]
fn animation_frame_does_not_materialize_text_rows() {
    let session = TerminalSession::new(20, 10, Box::new(NoopWakeup)).expect("session");
    session.apply_layout_geometry(20, 10, 9, 18).unwrap();
    let red = b64(&[255, 0, 0, 255]);
    let blue = b64(&[0, 0, 255, 255]);
    session.write_vt(
        format!("\x1b_Ga=t,f=32,s=1,v=1,i=3,q=2;{red}\x1b\\").as_bytes(),
    );
    session.write_vt(
        format!("\x1b_Ga=f,f=32,s=1,v=1,i=3,r=2,z=40,q=2;{blue}\x1b\\").as_bytes(),
    );
    session.write_vt(b"\x1b_Ga=p,i=3,p=300,q=2\x1b\\");
    session.write_vt(b"\x1b_Ga=a,i=3,s=3,q=2\x1b\\");
    let _ = session.render_snapshot().expect("first frame");
    session.mark_frame_complete().expect("complete");
    let before = session.snapshot_counters();

    let next = session.tick_kitty_animation(0).expect("tick").expect("deadline");
    assert!(next > 0);
    let frame = session.render_snapshot().expect("animation frame");
    session.mark_frame_complete().expect("complete");
    let counters = session.snapshot_counters();
    report("animation-frame", &counters);
    assert_eq!(frame.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
    assert!(
        frame.image_placements.iter().any(|p| p.image_id == 3),
        "the animated image stays placed"
    );
    assert_eq!(
        counters.rows_materialized, before.rows_materialized,
        "an image-only frame must not materialize text rows"
    );
    assert_eq!(counters.shape_line_calls, before.shape_line_calls);
    assert!(counters.kitty_animation_frames >= 1);
}

#[test]
fn dpi_only_change_does_not_resize_the_pty() {
    let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
    session.apply_layout_geometry(80, 24, 9, 18).unwrap();
    let before = session.snapshot_counters();
    session.apply_layout_geometry(80, 24, 14, 27).unwrap();
    let counters = session.snapshot_counters();
    report("dpi-only", &counters);
    assert_eq!(
        counters.ghostty_geometry_updates,
        before.ghostty_geometry_updates + 1
    );
    assert_eq!(
        counters.pty_grid_resize_requests,
        before.pty_grid_resize_requests,
        "DPI must never reach ResizePseudoConsole/TIOCSWINSZ"
    );
    assert_eq!(session.applied_geometry().cell_width_px, 14);
    assert_eq!(session.applied_geometry().cell_height_px, 27);
}

#[test]
fn many_images_stay_bounded_by_the_storage_cap() {
    use pain::terminal::ghostty::KittyConfig;
    use pain::terminal::session::SessionConfig;

    let session = TerminalSession::new_with_config(
        SessionConfig {
            cols: 20,
            rows: 10,
            kitty: KittyConfig {
                storage_limit_bytes: 64,
                ..KittyConfig::default()
            },
            ..SessionConfig::default()
        },
        Box::new(NoopWakeup),
    )
    .expect("session");
    session.apply_layout_geometry(20, 10, 9, 18).unwrap();
    for id in 1..=100u32 {
        place_static_image(&session, id);
    }
    let frame = session.render_snapshot().expect("frame");
    // Ghostty evicts old images under the cap; the placement list cannot grow
    // to one hundred visible images with a 64-byte budget.
    assert!(frame.image_placements.len() <= 16);
    let counters = session.snapshot_counters();
    report("bounded-images", &counters);
    assert!(counters.terminal_lock_hold_max_micros < 1_000_000);
}
