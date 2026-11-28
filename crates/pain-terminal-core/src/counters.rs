//! Ownership of terminal instrumentation counters.
//!
//! One leaf module owns `Counters`, its send-only handle and the snapshot.
//! Session/render code records through these types; no free atomics are
//! scattered across other modules.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Point-in-time copy of the instrumentation counters in docs/PERFORMANCE.md.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CountersSnapshot {
    // Terminal
    pub vt_write_calls: u64,
    pub vt_write_bytes: u64,
    pub vt_write_total_micros: u64,
    pub vt_write_max_micros: u64,
    pub terminal_lock_acquisitions: u64,
    pub terminal_lock_hold_total_micros: u64,
    pub terminal_lock_hold_max_micros: u64,
    // Wakeups
    pub output_generations: u64,
    pub wakeup_attempts: u64,
    pub wakeups_scheduled: u64,
    pub wakeups_consumed: u64,
    pub wakeups_coalesced: u64,
    // Render
    pub render_state_updates: u64,
    pub dirty_full: u64,
    pub dirty_partial: u64,
    pub dirty_clean: u64,
    pub dirty_rows_visited: u64,
    pub rows_rebuilt: u64,
    pub row_cache_hits: u64,
    pub text_batches_generated: u64,
    pub shape_line_calls: u64,
    pub shaped_utf8_bytes: u64,
    pub background_quads: u64,
    pub cursor_only_frames: u64,
    pub prepaint_micros: u64,
    pub paint_micros: u64,
    // Writer queue (recorded by WriterQueue, merged at snapshot time)
    pub writer_messages: u64,
    pub writer_queue_depth_max: u64,
    pub writer_bytes_queued: u64,
    pub writer_bytes_rejected: u64,
    pub title_changes: u64,
    // Resize: Ghostty geometry (cols/rows/cell pixels) and PTY grid are
    // independent decisions, so the two counters are deliberately separate.
    pub bounds_changes_observed: u64,
    pub grid_size_changes: u64,
    pub ghostty_geometry_updates: u64,
    pub pty_grid_resize_requests: u64,
    pub duplicate_resize_suppressed: u64,
    // Mouse routing
    pub mouse_events_offered: u64,
    pub mouse_events_encoded: u64,
    pub mouse_bytes_queued: u64,
    // Snapshot extraction and cache work
    pub cells_materialized: u64,
    pub rows_materialized: u64,
    pub row_cache_copies: u64,
    pub scroll_lock_acquisitions: u64,
    // Kitty graphics. Placement work is separate from row work by design.
    pub image_placement_list_rebuilds: u64,
    pub image_placement_cache_hits: u64,
    pub image_pixel_reads: u64,
    pub image_pixel_bytes_copied: u64,
    pub image_cache_misses: u64,
    pub image_generation_races: u64,
    pub image_texture_uploads: u64,
    pub image_texture_evictions: u64,
    pub image_copy_lock_max_micros: u64,
    pub kitty_animation_ticks: u64,
    pub kitty_animation_frames: u64,
}

#[derive(Default)]
pub(crate) struct Counters {
    pub(crate) vt_write_calls: AtomicU64,
    pub(crate) vt_write_bytes: AtomicU64,
    pub(crate) vt_write_total_micros: AtomicU64,
    pub(crate) vt_write_max_micros: AtomicU64,
    pub(crate) terminal_lock_acquisitions: AtomicU64,
    pub(crate) terminal_lock_hold_total_micros: AtomicU64,
    pub(crate) terminal_lock_hold_max_micros: AtomicU64,
    pub(crate) output_generations: AtomicU64,
    pub(crate) wakeup_attempts: AtomicU64,
    pub(crate) wakeups_scheduled: AtomicU64,
    pub(crate) wakeups_consumed: AtomicU64,
    pub(crate) wakeups_coalesced: AtomicU64,
    pub(crate) render_state_updates: AtomicU64,
    pub(crate) dirty_full: AtomicU64,
    pub(crate) dirty_partial: AtomicU64,
    pub(crate) dirty_clean: AtomicU64,
    pub(crate) dirty_rows_visited: AtomicU64,
    pub(crate) rows_rebuilt: AtomicU64,
    pub(crate) row_cache_hits: AtomicU64,
    pub(crate) text_batches_generated: AtomicU64,
    pub(crate) shape_line_calls: AtomicU64,
    shaped_utf8_bytes: AtomicU64,
    pub(crate) background_quads: AtomicU64,
    pub(crate) cursor_only_frames: AtomicU64,
    pub(crate) prepaint_micros: AtomicU64,
    pub(crate) paint_micros: AtomicU64,
    pub(crate) bounds_changes_observed: AtomicU64,
    pub(crate) grid_size_changes: AtomicU64,
    pub(crate) ghostty_geometry_updates: AtomicU64,
    pub(crate) pty_grid_resize_requests: AtomicU64,
    pub(crate) duplicate_resize_suppressed: AtomicU64,
    pub(crate) mouse_events_offered: AtomicU64,
    pub(crate) mouse_events_encoded: AtomicU64,
    pub(crate) mouse_bytes_queued: AtomicU64,
    pub(crate) cells_materialized: AtomicU64,
    pub(crate) rows_materialized: AtomicU64,
    pub(crate) row_cache_copies: AtomicU64,
    pub(crate) scroll_lock_acquisitions: AtomicU64,
    pub(crate) title_changes: AtomicU64,
    pub(crate) image_placement_list_rebuilds: AtomicU64,
    pub(crate) image_placement_cache_hits: AtomicU64,
    pub(crate) image_pixel_reads: AtomicU64,
    pub(crate) image_pixel_bytes_copied: AtomicU64,
    pub(crate) image_cache_misses: AtomicU64,
    pub(crate) image_generation_races: AtomicU64,
    pub(crate) image_texture_uploads: AtomicU64,
    pub(crate) image_texture_evictions: AtomicU64,
    pub(crate) image_copy_lock_max_micros: AtomicU64,
    pub(crate) kitty_animation_ticks: AtomicU64,
    pub(crate) kitty_animation_frames: AtomicU64,
}

impl Counters {
    pub(crate) fn add(counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    pub(crate) fn record_max(counter: &AtomicU64, value: u64) {
        let mut prev = counter.load(Ordering::Relaxed);
        while value > prev
            && counter
                .compare_exchange_weak(prev, value, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            prev = counter.load(Ordering::Relaxed);
        }
    }

    pub(crate) fn micros(start: Instant) -> u64 {
        u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    pub(crate) fn snapshot(&self) -> CountersSnapshot {
        macro_rules! get {
            ($field:ident) => {
                self.$field.load(Ordering::Relaxed)
            };
        }
        CountersSnapshot {
            vt_write_calls: get!(vt_write_calls),
            vt_write_bytes: get!(vt_write_bytes),
            vt_write_total_micros: get!(vt_write_total_micros),
            vt_write_max_micros: get!(vt_write_max_micros),
            terminal_lock_acquisitions: get!(terminal_lock_acquisitions),
            terminal_lock_hold_total_micros: get!(terminal_lock_hold_total_micros),
            terminal_lock_hold_max_micros: get!(terminal_lock_hold_max_micros),
            output_generations: get!(output_generations),
            wakeup_attempts: get!(wakeup_attempts),
            wakeups_scheduled: get!(wakeups_scheduled),
            wakeups_consumed: get!(wakeups_consumed),
            wakeups_coalesced: get!(wakeups_coalesced),
            render_state_updates: get!(render_state_updates),
            dirty_full: get!(dirty_full),
            dirty_partial: get!(dirty_partial),
            dirty_clean: get!(dirty_clean),
            dirty_rows_visited: get!(dirty_rows_visited),
            rows_rebuilt: get!(rows_rebuilt),
            row_cache_hits: get!(row_cache_hits),
            text_batches_generated: get!(text_batches_generated),
            shape_line_calls: get!(shape_line_calls),
            shaped_utf8_bytes: get!(shaped_utf8_bytes),
            background_quads: get!(background_quads),
            cursor_only_frames: get!(cursor_only_frames),
            prepaint_micros: get!(prepaint_micros),
            paint_micros: get!(paint_micros),
            writer_messages: 0, // merged from WriterQueue in snapshot_counters
            writer_queue_depth_max: 0,
            writer_bytes_queued: 0,
            writer_bytes_rejected: 0, // merged from WriterQueue in snapshot_counters
            title_changes: get!(title_changes),
            bounds_changes_observed: get!(bounds_changes_observed),
            grid_size_changes: get!(grid_size_changes),
            ghostty_geometry_updates: get!(ghostty_geometry_updates),
            pty_grid_resize_requests: get!(pty_grid_resize_requests),
            duplicate_resize_suppressed: get!(duplicate_resize_suppressed),
            mouse_events_offered: get!(mouse_events_offered),
            mouse_events_encoded: get!(mouse_events_encoded),
            mouse_bytes_queued: get!(mouse_bytes_queued),
            cells_materialized: get!(cells_materialized),
            rows_materialized: get!(rows_materialized),
            row_cache_copies: get!(row_cache_copies),
            scroll_lock_acquisitions: get!(scroll_lock_acquisitions),
            image_placement_list_rebuilds: get!(image_placement_list_rebuilds),
            image_placement_cache_hits: get!(image_placement_cache_hits),
            image_pixel_reads: get!(image_pixel_reads),
            image_pixel_bytes_copied: get!(image_pixel_bytes_copied),
            image_cache_misses: get!(image_cache_misses),
            image_generation_races: get!(image_generation_races),
            image_texture_uploads: get!(image_texture_uploads),
            image_texture_evictions: get!(image_texture_evictions),
            image_copy_lock_max_micros: get!(image_copy_lock_max_micros),
            kitty_animation_ticks: get!(kitty_animation_ticks),
            kitty_animation_frames: get!(kitty_animation_frames),
        }
    }
}

/// Send-only view of the counters for renderer code that cannot borrow the
/// session (e.g. a GPUI element during prepaint/paint).
pub struct CountersHandle(pub(crate) Arc<Counters>);

impl CountersHandle {
    pub fn rows_rebuilt(&self, count: u64) {
        Counters::add(&self.0.rows_rebuilt, count);
    }
    pub fn row_cache_hits(&self, count: u64) {
        Counters::add(&self.0.row_cache_hits, count);
    }
    pub fn row_cache_copies(&self, count: u64) {
        Counters::add(&self.0.row_cache_copies, count);
    }
    pub fn text_batches(&self, count: u64) {
        Counters::add(&self.0.text_batches_generated, count);
    }
    pub fn shape_line(&self, bytes: usize) {
        Counters::add(&self.0.shape_line_calls, 1);
        Counters::add(&self.0.shaped_utf8_bytes, bytes as u64);
    }
    pub fn background_quads(&self, count: u64) {
        Counters::add(&self.0.background_quads, count);
    }
    pub fn prepaint_micros(&self, micros: u64) {
        Counters::add(&self.0.prepaint_micros, micros);
    }
    pub fn paint_micros(&self, micros: u64) {
        Counters::add(&self.0.paint_micros, micros);
    }
    pub fn image_texture_upload(&self) {
        Counters::add(&self.0.image_texture_uploads, 1);
    }
    pub fn image_texture_eviction(&self) {
        Counters::add(&self.0.image_texture_evictions, 1);
    }
    pub fn image_placement_list_rebuild(&self) {
        Counters::add(&self.0.image_placement_list_rebuilds, 1);
    }
    pub fn image_placement_cache_hit(&self) {
        Counters::add(&self.0.image_placement_cache_hits, 1);
    }
}
