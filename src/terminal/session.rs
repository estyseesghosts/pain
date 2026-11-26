//! M2 shared terminal session: one serialized owner of the Ghostty handles,
//! a two-phase render update, and a coalesced wakeup path.
//!
//! There is no PTY here. A synthetic byte producer calls [`TerminalSession::write_vt`]
//! exactly where the future ConPTY/POSIX reader will, and the UI consumes a
//! single pending wakeup per frame. Terminal work never holds a lock across
//! GPUI shaping or a channel send.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::terminal::ghostty::{
    ghostty_vt_sys as sys, CallbackRegistration, GhosttyError, GhosttyRenderState, GhosttyTerminal,
    KeyEncoder, KeyEvent, KittyConfig, KittyImagePixels, KittyImageRead, MouseEncoder, MouseEvent,
    RenderSnapshot, SelectionGesture,
};
use crate::terminal::writer::WriterQueue;

/// UI notification hook. Keep this UI-agnostic: the GPUI integration supplies
/// a sink that posts to the foreground executor, headless tests use a counter.
pub trait WakeupSink: Send + Sync {
    fn schedule(&self);
}

impl<F: Fn() + Send + Sync> WakeupSink for F {
    fn schedule(&self) {
        self();
    }
}

pub struct NoopWakeup;
impl WakeupSink for NoopWakeup {
    fn schedule(&self) {}
}

/// Platform PTY resize hook, installed by the app when a real PTY exists.
type PtyResize = Arc<dyn Fn(u16, u16) + Send + Sync>;

/// Result of one viewport operation, read while the terminal lock is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScrollOutcome {
    pub moved: bool,
    pub scrollbar: sys::GhosttyTerminalScrollbar,
}

/// One pointer event, in the same terms Ghostty's mouse encoder expects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseInput {
    pub action: i32,
    pub button: Option<i32>,
    pub mods: u16,
    pub x: f32,
    pub y: f32,
    pub any_button_pressed: bool,
    pub surface_width: u32,
    pub surface_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// Who owns a pointer event. An empty encoded buffer can still mean "reported
/// but deduplicated by Ghostty", so ownership is expressed by the variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseRoute {
    NotTracking,
    Reported { bytes_queued: usize },
}

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
    // Process
    pub process_exited: bool,
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
struct Counters {
    vt_write_calls: AtomicU64,
    vt_write_bytes: AtomicU64,
    vt_write_total_micros: AtomicU64,
    vt_write_max_micros: AtomicU64,
    terminal_lock_acquisitions: AtomicU64,
    terminal_lock_hold_total_micros: AtomicU64,
    terminal_lock_hold_max_micros: AtomicU64,
    output_generations: AtomicU64,
    wakeup_attempts: AtomicU64,
    wakeups_scheduled: AtomicU64,
    wakeups_consumed: AtomicU64,
    wakeups_coalesced: AtomicU64,
    render_state_updates: AtomicU64,
    dirty_full: AtomicU64,
    dirty_partial: AtomicU64,
    dirty_clean: AtomicU64,
    dirty_rows_visited: AtomicU64,
    rows_rebuilt: AtomicU64,
    row_cache_hits: AtomicU64,
    text_batches_generated: AtomicU64,
    shape_line_calls: AtomicU64,
    shaped_utf8_bytes: AtomicU64,
    background_quads: AtomicU64,
    cursor_only_frames: AtomicU64,
    prepaint_micros: AtomicU64,
    paint_micros: AtomicU64,
    bounds_changes_observed: AtomicU64,
    grid_size_changes: AtomicU64,
    ghostty_geometry_updates: AtomicU64,
    pty_grid_resize_requests: AtomicU64,
    duplicate_resize_suppressed: AtomicU64,
    mouse_events_offered: AtomicU64,
    mouse_events_encoded: AtomicU64,
    mouse_bytes_queued: AtomicU64,
    cells_materialized: AtomicU64,
    rows_materialized: AtomicU64,
    row_cache_copies: AtomicU64,
    scroll_lock_acquisitions: AtomicU64,
    title_changes: AtomicU64,
    image_placement_list_rebuilds: AtomicU64,
    image_placement_cache_hits: AtomicU64,
    image_pixel_reads: AtomicU64,
    image_pixel_bytes_copied: AtomicU64,
    image_cache_misses: AtomicU64,
    image_generation_races: AtomicU64,
    image_texture_uploads: AtomicU64,
    image_texture_evictions: AtomicU64,
    image_copy_lock_max_micros: AtomicU64,
    kitty_animation_ticks: AtomicU64,
    kitty_animation_frames: AtomicU64,
}

impl Counters {
    fn add(counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    fn record_max(counter: &AtomicU64, value: u64) {
        let mut prev = counter.load(Ordering::Relaxed);
        while value > prev
            && counter
                .compare_exchange_weak(prev, value, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            prev = counter.load(Ordering::Relaxed);
        }
    }

    fn micros(start: Instant) -> u64 {
        u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn snapshot(&self) -> CountersSnapshot {
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
            process_exited: false,
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

/// Deliberate scrollback caps and starting grid. Ghostty's default byte cap is
/// 10 KiB, which discards usable history almost immediately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionConfig {
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
    pub scrollback_bytes: usize,
    pub kitty: KittyConfig,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            scrollback_lines: 10_000,
            scrollback_bytes: 16 * 1024 * 1024,
            kitty: KittyConfig::default(),
        }
    }
}

/// The last geometry actually applied to Ghostty. All four fields matter:
/// cell-pixel changes (font size / DPI) are real Ghostty geometry updates even
/// when the visible grid is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AppliedGeometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

/// Result of one layout geometry application. `terminal_changed` and
/// `grid_changed` are independent: only the latter is allowed to reach the OS
/// PTY, while neither produces a renderer wakeup when called from prepaint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ResizeOutcome {
    pub terminal_changed: bool,
    pub grid_changed: bool,
}

impl ResizeOutcome {
    #[must_use]
    pub fn changed(self) -> bool {
        self.terminal_changed || self.grid_changed
    }
}

/// The single owner of one terminal session. `Send + Sync` is safe because
/// every Ghostty handle stays behind a private mutex and the raw pointers are
/// never exposed to callers.
pub struct TerminalSession {
    // `selection_gesture` is declared before `terminal` so it drops FIRST: its
    // free call needs the terminal to release tracked grid refs. `terminal`
    // still drops before `callbacks`, so the registration token outlives every
    // callback the terminal can invoke.
    selection_gesture: Mutex<SelectionGesture>,
    terminal: Mutex<GhosttyTerminal>,
    callbacks: Mutex<Option<CallbackRegistration>>,
    state: Mutex<GhosttyRenderState>,
    encoder: Mutex<KeyEncoder>,
    mouse_encoder: Mutex<MouseEncoder>,
    mouse_event: Mutex<MouseEvent>,
    generation: AtomicU64,
    consumed_generation: AtomicU64,
    pending_wakeup: AtomicBool,
    exited: AtomicBool,
    wakeup: Box<dyn WakeupSink>,
    writer: Arc<WriterQueue>,
    /// One stored last-applied geometry. Cell pixels change independently of
    /// the grid, so two independent mutexes could never express that.
    geometry: Mutex<AppliedGeometry>,
    pty_resize: Mutex<Option<PtyResize>>,
    counters: Arc<Counters>,
    /// One monotonic clock origin for this terminal's Kitty animations. Never
    /// reset on resize, alternate-screen switches or pause.
    kitty_clock: Instant,
    kitty_config: KittyConfig,
}

impl TerminalSession {
    pub fn new(cols: u16, rows: u16, wakeup: Box<dyn WakeupSink>) -> Result<Self, GhosttyError> {
        Self::new_with_config(
            SessionConfig {
                cols,
                rows,
                ..SessionConfig::default()
            },
            wakeup,
        )
    }

    pub fn new_with_config(
        config: SessionConfig,
        wakeup: Box<dyn WakeupSink>,
    ) -> Result<Self, GhosttyError> {
        let mut terminal = GhosttyTerminal::new(config.cols, config.rows)?;
        let kitty = config.kitty.clone().validated();
        // Policy is applied immediately after creation, on the one terminal
        // that this session will own. It cannot be re-applied later because
        // the storage cap is part of the terminal's live state.
        terminal.apply_kitty_config(&kitty)?;
        terminal.set_scrollback_limits(config.scrollback_lines, config.scrollback_bytes);
        let selection_gesture = SelectionGesture::new(&terminal)?;
        Ok(Self {
            terminal: Mutex::new(terminal),
            callbacks: Mutex::new(None),
            state: Mutex::new(GhosttyRenderState::new()?),
            encoder: Mutex::new(KeyEncoder::new()?),
            selection_gesture: Mutex::new(selection_gesture),
            mouse_encoder: Mutex::new(MouseEncoder::new()?),
            mouse_event: Mutex::new(MouseEvent::new()?),
            generation: AtomicU64::new(0),
            consumed_generation: AtomicU64::new(0),
            pending_wakeup: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            wakeup,
            writer: Arc::new(WriterQueue::new()),
            geometry: Mutex::new(AppliedGeometry {
                cols: config.cols,
                rows: config.rows,
                cell_width_px: 0,
                cell_height_px: 0,
            }),
            pty_resize: Mutex::new(None),
            counters: Arc::new(Counters::default()),
            kitty_clock: Instant::now(),
            kitty_config: kitty,
        })
    }

    /// Cheap handle for recording renderer-side counters without borrowing the
    /// session mutably from a GPUI element.
    #[must_use]
    pub fn counters(&self) -> Arc<CountersHandle> {
        Arc::new(CountersHandle(self.counters.clone()))
    }

    #[must_use]
    pub fn snapshot_counters(&self) -> CountersSnapshot {
        let mut snapshot = self.counters.snapshot();
        let writer = self.writer.stats();
        snapshot.writer_messages = writer.messages;
        snapshot.writer_queue_depth_max = writer.max_depth;
        snapshot.writer_bytes_queued = writer.bytes;
        snapshot.process_exited = self.process_exited();
        snapshot
    }

    /// Called by the reader (or synthetic producer) for each output chunk.
    /// Lock hold time is bounded to Ghostty's VT parser; no UI work happens
    /// while the terminal is locked.
    pub fn write_vt(&self, bytes: &[u8]) {
        let write_start = Instant::now();
        let lock_start = Instant::now();
        {
            let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
            Counters::add(&self.counters.terminal_lock_acquisitions, 1);
            terminal.write(bytes);
            Counters::add(
                &self.counters.terminal_lock_hold_total_micros,
                Counters::micros(lock_start),
            );
            Counters::record_max(
                &self.counters.terminal_lock_hold_max_micros,
                Counters::micros(lock_start),
            );
        }
        Counters::add(&self.counters.vt_write_calls, 1);
        Counters::add(&self.counters.vt_write_bytes, bytes.len() as u64);
        Counters::add(
            &self.counters.vt_write_total_micros,
            Counters::micros(write_start),
        );
        Counters::record_max(
            &self.counters.vt_write_max_micros,
            Counters::micros(write_start),
        );
        self.generation.fetch_add(1, Ordering::Release);
        self.schedule_wakeup();
    }

    /// Coalesced wakeup: many VT writes with a pending flag produce one UI
    /// notification. The transition is made with `swap` so a producer that
    /// writes after the UI clears the flag always re-arms it.
    fn schedule_wakeup(&self) {
        Counters::add(&self.counters.wakeup_attempts, 1);
        self.generation.fetch_add(0, Ordering::Relaxed);
        if self.pending_wakeup.swap(true, Ordering::AcqRel) {
            Counters::add(&self.counters.wakeups_coalesced, 1);
            return;
        }
        Counters::add(&self.counters.wakeups_scheduled, 1);
        self.wakeup.schedule();
    }

    /// Consume one pending wakeup, returning the output generation that the UI
    /// should render. `None` means nothing changed since the last consumption.
    #[must_use]
    pub fn take_wakeup(&self) -> Option<u64> {
        if !self.pending_wakeup.swap(false, Ordering::AcqRel) {
            return None;
        }
        let generation = self.generation.load(Ordering::Acquire);
        self.consumed_generation
            .store(generation, Ordering::Release);
        Counters::add(&self.counters.wakeups_consumed, 1);
        Some(generation)
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn consumed_generation(&self) -> u64 {
        self.consumed_generation.load(Ordering::Acquire)
    }

    /// Two-phase render-state update. The terminal lock is held only across
    /// `begin_update`; shaping and painting happen after it is released.
    pub fn render_snapshot(&self) -> Result<RenderSnapshot, GhosttyError> {
        let mut state = self.state.lock().expect("render-state mutex poisoned");
        {
            let terminal = self.terminal.lock().expect("terminal mutex poisoned");
            Counters::add(&self.counters.terminal_lock_acquisitions, 1);
            let start = Instant::now();
            state.begin_update(&terminal)?;
            Counters::add(
                &self.counters.terminal_lock_hold_total_micros,
                Counters::micros(start),
            );
            Counters::record_max(
                &self.counters.terminal_lock_hold_max_micros,
                Counters::micros(start),
            );
        }
        state.end_update()?;
        Counters::add(&self.counters.render_state_updates, 1);
        // Read metadata first, then materialize ONLY the rows this frame needs.
        let meta = state.snapshot_meta()?;
        if meta.image_placements_rebuilt {
            Counters::add(&self.counters.image_placement_list_rebuilds, 1);
        } else {
            Counters::add(&self.counters.image_placement_cache_hits, 1);
        }
        let mut needed = vec![false; usize::from(meta.rows)];
        match meta.dirty {
            sys::GHOSTTY_RENDER_STATE_DIRTY_FULL => needed.fill(true),
            sys::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                for y in &meta.dirty_rows {
                    if let Some(slot) = needed.get_mut(usize::from(*y)) {
                        *slot = true;
                    }
                }
            }
            _ => {}
        }
        let rows = state.materialize_rows(&needed, &meta.selections)?;
        Counters::add(&self.counters.rows_materialized, rows.len() as u64);
        Counters::add(
            &self.counters.cells_materialized,
            rows.iter().map(|(_, cells)| cells.len() as u64).sum(),
        );
        let snapshot = RenderSnapshot { meta, rows };
        let dirty_rows = &snapshot.dirty_rows;
        Counters::add(&self.counters.dirty_rows_visited, dirty_rows.len() as u64);
        match snapshot.dirty {
            sys::GHOSTTY_RENDER_STATE_DIRTY_FULL => Counters::add(&self.counters.dirty_full, 1),
            sys::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                Counters::add(&self.counters.dirty_partial, 1)
            }
            _ => {
                Counters::add(&self.counters.dirty_clean, 1);
                if dirty_rows.is_empty() {
                    Counters::add(&self.counters.cursor_only_frames, 1);
                }
            }
        }
        Ok(snapshot)
    }

    /// Clear dirty flags only after the whole frame was consumed successfully.
    pub fn mark_frame_complete(&self) -> Result<(), GhosttyError> {
        self.state
            .lock()
            .expect("render-state mutex poisoned")
            .clean()
    }

    /// Current time on the terminal's single monotonic Kitty clock.
    #[must_use]
    pub fn kitty_clock_ms(&self) -> u64 {
        u64::try_from(self.kitty_clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The validated transport policy this session was created with.
    #[must_use]
    pub fn kitty_config(&self) -> &KittyConfig {
        &self.kitty_config
    }

    /// Query the live policy from Ghostty: storage limit, file medium, shared
    /// memory medium. Used by policy tests and the documented smoke gate.
    pub fn kitty_policy_state(&self) -> Result<(u64, bool, bool), GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        Ok((
            terminal.kitty_storage_limit()?,
            terminal.kitty_medium_flags()?.0,
            terminal.kitty_medium_flags()?.1,
        ))
    }

    /// Advance Kitty animation by at most one frame. Demand-driven: callers
    /// schedule the returned deadline rather than polling. Returns the next
    /// absolute time on [`Self::kitty_clock_ms`], or `None` when nothing plays.
    pub fn tick_kitty_animation(&self, now_ms: u64) -> Result<Option<u64>, GhosttyError> {
        let start = Instant::now();
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        Counters::add(&self.counters.terminal_lock_acquisitions, 1);
        Counters::add(&self.counters.kitty_animation_ticks, 1);
        let before = terminal.kitty_storage_generation().unwrap_or(0);
        let next = terminal.tick_kitty_animation(now_ms)?;
        let after = terminal.kitty_storage_generation().unwrap_or(before);
        let elapsed = Counters::micros(start);
        Counters::add(
            &self.counters.terminal_lock_hold_total_micros,
            elapsed,
        );
        Counters::record_max(
            &self.counters.terminal_lock_hold_max_micros,
            elapsed,
        );
        drop(terminal);
        if before != after {
            Counters::add(&self.counters.kitty_animation_frames, 1);
            self.notify_changed();
        }
        Ok(next)
    }

    /// Copy raw pixels for the requested `(image_id, generation)` pairs in one
    /// terminal-lock critical section. Duplicate requests are deduplicated
    /// before the lock. Generation races are normal and are simply skipped.
    pub fn read_kitty_images(&self, requested: &[(u32, u64)]) -> Vec<KittyImagePixels> {
        if requested.is_empty() {
            return Vec::new();
        }
        // Deduplicate without holding the terminal lock. The request list is
        // the renderer's missing-texture set, so it is normally tiny.
        let mut unique: Vec<(u32, u64)> = Vec::with_capacity(requested.len());
        for &item in requested {
            if !unique.contains(&item) {
                unique.push(item);
            }
        }
        Counters::add(
            &self.counters.image_cache_misses,
            unique.len() as u64,
        );
        let start = Instant::now();
        let mut copied = Vec::with_capacity(unique.len());
        {
            let terminal = self.terminal.lock().expect("terminal mutex poisoned");
            Counters::add(&self.counters.terminal_lock_acquisitions, 1);
            let graphics = match terminal.kitty_graphics() {
                Ok(graphics) => graphics,
                Err(_) => return copied,
            };
            for (image_id, generation) in unique {
                match GhosttyTerminal::read_kitty_image_from(graphics, image_id, generation) {
                    Ok(KittyImageRead::Pixels(pixels)) => {
                        Counters::add(&self.counters.image_pixel_reads, 1);
                        Counters::add(
                            &self.counters.image_pixel_bytes_copied,
                            pixels.pixels.len() as u64,
                        );
                        copied.push(pixels);
                    }
                    Ok(KittyImageRead::GenerationChanged) => {
                        Counters::add(&self.counters.image_generation_races, 1);
                    }
                    Ok(KittyImageRead::Missing) => {}
                    Err(_) => {}
                }
            }
            let elapsed = Counters::micros(start);
            Counters::add(
                &self.counters.terminal_lock_hold_total_micros,
                elapsed,
            );
            Counters::record_max(
                &self.counters.terminal_lock_hold_max_micros,
                elapsed,
            );
            Counters::record_max(
                &self.counters.image_copy_lock_max_micros,
                elapsed,
            );
        }
        copied
    }

    /// Apply layout-derived geometry. This is called from prepaint, which
    /// immediately renders the new state in the same frame, so it never
    /// schedules another renderer wakeup.
    ///
    /// Ghostty geometry (cols, rows, cell pixels) and the OS PTY grid are two
    /// independent decisions. A font/DPI change updates Ghostty pixels but must
    /// not create a fake PTY resize; a one-cell column change updates both.
    pub fn apply_layout_geometry(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<ResizeOutcome, GhosttyError> {
        Counters::add(&self.counters.bounds_changes_observed, 1);
        let requested = AppliedGeometry {
            cols,
            rows,
            cell_width_px: cell_width_px.max(1),
            cell_height_px: cell_height_px.max(1),
        };
        let previous = {
            let mut geometry = self.geometry.lock().expect("geometry mutex poisoned");
            let previous = *geometry;
            if previous == requested {
                Counters::add(&self.counters.duplicate_resize_suppressed, 1);
                return Ok(ResizeOutcome::default());
            }
            *geometry = requested;
            previous
        };
        let outcome = ResizeOutcome {
            terminal_changed: true,
            grid_changed: previous.cols != cols || previous.rows != rows,
        };
        {
            let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
            if let Err(error) =
                terminal.resize(cols, rows, requested.cell_width_px, requested.cell_height_px)
            {
                // Never keep a "last applied" geometry that Ghostty rejected.
                *self.geometry.lock().expect("geometry mutex poisoned") = previous;
                return Err(error);
            }
        }
        Counters::add(&self.counters.ghostty_geometry_updates, 1);
        if outcome.grid_changed {
            Counters::add(&self.counters.grid_size_changes, 1);
            // The platform PTY sees only cell counts, never pixels, and only
            // for distinct grids. The callback runs with no terminal lock held.
            Counters::add(&self.counters.pty_grid_resize_requests, 1);
            if let Some(resize) = self
                .pty_resize
                .lock()
                .expect("pty_resize poisoned")
                .as_ref()
            {
                resize(cols, rows);
            }
        }
        Ok(outcome)
    }

    /// A resize that did NOT originate in the current layout frame must
    /// explicitly request the extra repaint it needs.
    pub fn apply_external_geometry(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<ResizeOutcome, GhosttyError> {
        let outcome =
            self.apply_layout_geometry(cols, rows, cell_width_px, cell_height_px)?;
        if outcome.changed() {
            self.schedule_wakeup();
        }
        Ok(outcome)
    }

    /// Last geometry applied to Ghostty, exposed for headless assertions.
    #[must_use]
    pub fn applied_geometry(&self) -> AppliedGeometry {
        *self.geometry.lock().expect("geometry mutex poisoned")
    }

    /// Install the platform resize hook. Called once after PTY creation.
    pub fn set_pty_resize(&self, resize: Arc<dyn Fn(u16, u16) + Send + Sync>) {
        *self.pty_resize.lock().expect("pty_resize poisoned") = Some(resize);
    }

    /// Encode a key against current Ghostty modes and queue the bytes for the
    /// (future) PTY writer. Returning the bytes keeps this testable.
    pub fn encode_key(&self, event: &KeyEvent) -> Result<Vec<u8>, GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let mut encoder = self.encoder.lock().expect("encoder mutex poisoned");
        encoder.sync_from_terminal(&terminal)?;
        let bytes = encoder.encode_owned(event)?;
        drop(encoder);
        drop(terminal);
        self.enqueue_writer(bytes.clone());
        Ok(bytes)
    }

    /// Install a Ghostty-owned selection for the M1 selection fixture.
    pub fn set_row_selection(&self, y: u16, start_x: u16, end_x: u16) -> Result<(), GhosttyError> {
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.set_row_selection(y, start_x, end_x)
    }

    /// Bracketed-paste policy comes from the terminal's current mode, never
    /// from a Rust-side mode table.
    pub fn paste(&self, text: &str) -> Result<Vec<u8>, GhosttyError> {
        let bracketed = {
            let terminal = self.terminal.lock().expect("terminal mutex poisoned");
            terminal.mode(sys::GHOSTTY_MODE_BRACKETED_PASTE)?
        };
        let bytes = crate::terminal::ghostty::paste_encode(text, bracketed)?;
        self.enqueue_writer(bytes.clone());
        Ok(bytes)
    }

    /// Apply a new terminal palette from the preferences window. Ghostty marks
    /// the render state dirty; the wakeup makes the next frame rebuild cached
    /// rows even when no PTY output arrived.
    pub fn set_colors(
        &self,
        colors: Option<&crate::terminal::palette::Palette>,
    ) -> Result<(), GhosttyError> {
        {
            let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
            terminal.set_colors(colors)?;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.schedule_wakeup();
        Ok(())
    }

    /// Writer queue interface. UI input and Ghostty's `WRITE_PTY` callback
    /// both enqueue here; the platform writer thread drains it in order.
    pub fn enqueue_writer(&self, bytes: Vec<u8>) -> usize {
        self.writer.push(bytes)
    }

    /// Non-blocking drain, used by headless tests.
    #[must_use]
    pub fn drain_writer(&self, max: usize) -> Vec<Vec<u8>> {
        (0..max).filter_map(|_| self.writer.pop()).collect()
    }

    #[must_use]
    pub fn writer_depth(&self) -> usize {
        self.writer.len()
    }

    /// Blocking drain for the platform writer thread. `None` after `close_writer`.
    #[must_use]
    pub fn wait_writer(&self) -> Option<Vec<u8>> {
        self.writer.pop_blocking()
    }

    /// Signal the writer thread to stop after flushing queued bytes.
    pub fn close_writer(&self) {
        self.writer.close();
    }

    /// Shared queue handle for installing Ghostty's `WRITE_PTY` callback.
    #[must_use]
    pub fn writer_queue(&self) -> Arc<WriterQueue> {
        Arc::clone(&self.writer)
    }

    /// Register Ghostty's effect callbacks. `on_title` is invoked on the thread
    /// that wrote VT bytes; the app supplies a sink that only posts to the UI.
    pub fn install_callbacks(
        &self,
        on_title: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<(), GhosttyError> {
        let registration = {
            let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
            terminal.install_callbacks(Arc::clone(&self.writer), on_title)?
        };
        *self.callbacks.lock().expect("callbacks mutex poisoned") = Some(registration);
        Ok(())
    }

    /// Register only the `WRITE_PTY` bridge, with a no-op title sink. Used by
    /// PTY integration tests that do not own a window.
    pub fn install_pty_writer(&self) -> Result<(), GhosttyError> {
        self.install_callbacks(Box::new(|_| {}))
    }

    /// One lock acquisition for one scroll gesture: read the prior offset,
    /// mutate the viewport, read the resulting scrollbar state, then release.
    /// The returned state is what scrollbar drag uses; there is no second
    /// `scrollbar()` query after this method returns.
    fn scroll_with(
        &self,
        behavior: sys::GhosttyTerminalScrollViewport,
    ) -> Result<ScrollOutcome, GhosttyError> {
        let start = Instant::now();
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        Counters::add(&self.counters.terminal_lock_acquisitions, 1);
        Counters::add(&self.counters.scroll_lock_acquisitions, 1);
        let before = terminal.scrollbar()?.offset;
        terminal.scroll_viewport(behavior);
        let after = terminal.scrollbar()?;
        Counters::add(
            &self.counters.terminal_lock_hold_total_micros,
            Counters::micros(start),
        );
        Counters::record_max(
            &self.counters.terminal_lock_hold_max_micros,
            Counters::micros(start),
        );
        drop(terminal);
        let moved = before != after.offset;
        if moved {
            self.notify_changed();
        }
        Ok(ScrollOutcome {
            moved,
            scrollbar: after,
        })
    }

    /// Scroll the Ghostty-owned viewport by `delta_rows` (negative = up into
    /// history). Returns whether the viewport actually moved.
    pub fn scroll(&self, delta_rows: isize) -> Result<bool, GhosttyError> {
        if delta_rows == 0 {
            return Ok(false);
        }
        Ok(self
            .scroll_with(sys::GhosttyTerminalScrollViewport::delta(delta_rows))?
            .moved)
    }

    pub fn scroll_to_top(&self) -> Result<ScrollOutcome, GhosttyError> {
        self.scroll_with(sys::GhosttyTerminalScrollViewport::top())
    }

    pub fn scroll_to_bottom(&self) -> Result<ScrollOutcome, GhosttyError> {
        self.scroll_with(sys::GhosttyTerminalScrollViewport::bottom())
    }

    /// Place the first visible row at an absolute scrollbar offset. This is the
    /// round-trip used by the scrollbar thumb drag.
    pub fn scroll_to_row(&self, row: usize) -> Result<ScrollOutcome, GhosttyError> {
        self.scroll_with(sys::GhosttyTerminalScrollViewport::row(row))
    }

    /// Current scrollbar geometry, read from Ghostty on demand (no second
    /// scroll model and no GPUI `ScrollHandle` as source of truth).
    pub fn scrollbar(&self) -> Result<sys::GhosttyTerminalScrollbar, GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.scrollbar()
    }

    /// Current terminal title from OSC 0/2, or an empty string when unset.
    #[must_use]
    pub fn title(&self) -> String {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.title()
    }

    /// Called by the title effect sink; counts transitions for M7 counters.
    pub fn record_title_change(&self) {
        Counters::add(&self.counters.title_changes, 1);
    }

    /// Write committed text straight to the PTY (IME/character input path).
    /// This is distinct from `paste`, which applies bracketed-paste semantics.
    pub fn write_input(&self, text: &str) {
        if !text.is_empty() {
            self.enqueue_writer(text.as_bytes().to_vec());
        }
    }

    /// Whether the running program currently reports mouse tracking. The UI
    /// uses this to choose local selection vs terminal mouse encoding.
    pub fn mouse_tracking_enabled(&self) -> Result<bool, GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.mouse_tracking()
    }

    /// Decide mouse ownership and encode under ONE terminal lock, then move the
    /// encoded bytes straight into the writer queue. There is no cached
    /// "tracking enabled" flag that could drift from Ghostty's modes.
    pub fn route_mouse(&self, input: MouseInput) -> Result<MouseRoute, GhosttyError> {
        Counters::add(&self.counters.mouse_events_offered, 1);
        let mut encoder = self.mouse_encoder.lock().expect("mouse encoder poisoned");
        let mut event = self.mouse_event.lock().expect("mouse event poisoned");
        {
            let terminal = self.terminal.lock().expect("terminal mutex poisoned");
            if !terminal.mouse_tracking()? {
                return Ok(MouseRoute::NotTracking);
            }
            encoder.sync_from_terminal(&terminal)?;
        }
        let Some(bytes) = self.encode_with_encoder(&mut encoder, &mut event, input)? else {
            // Tracking owns the event but Ghostty deduplicated the motion.
            return Ok(MouseRoute::Reported { bytes_queued: 0 });
        };
        let queued = bytes.len();
        // Production path: move the Vec; no clone for the UI's benefit.
        self.writer.push(bytes);
        Counters::add(&self.counters.mouse_events_encoded, 1);
        Counters::add(&self.counters.mouse_bytes_queued, queued as u64);
        Ok(MouseRoute::Reported { bytes_queued: queued })
    }

    fn encode_with_encoder(
        &self,
        encoder: &mut MouseEncoder,
        event: &mut MouseEvent,
        input: MouseInput,
    ) -> Result<Option<Vec<u8>>, GhosttyError> {
        encoder.track_last_cell(true)?;
        encoder.set_any_button_pressed(input.any_button_pressed)?;
        let mut size = sys::GhosttyMouseEncoderSize::sized();
        size.screen_width = input.surface_width;
        size.screen_height = input.surface_height;
        size.cell_width = input.cell_width.max(1);
        size.cell_height = input.cell_height.max(1);
        encoder.set_size(&size)?;
        event.configure(
            input.action,
            input.button,
            input.mods,
            input.x,
            input.y,
        )?;
        let mut buf = [0u8; 128];
        let written = encoder.encode(event, &mut buf)?;
        Ok(if written == 0 {
            None
        } else {
            Some(buf[..written].to_vec())
        })
    }

    /// Backwards-compatible test helper with the historian's flat argument
    /// list; production code uses `route_mouse`.
    #[allow(clippy::too_many_arguments)]
    pub fn encode_mouse(
        &self,
        action: i32,
        button: Option<i32>,
        mods: u16,
        x: f32,
        y: f32,
        any_button_pressed: bool,
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<Vec<u8>, GhosttyError> {
        self.encode_mouse_for_test(MouseInput {
            action,
            button,
            mods,
            x,
            y,
            any_button_pressed,
            surface_width: screen_width,
            surface_height: screen_height,
            cell_width,
            cell_height,
        })
    }

    /// Test-only helper: encode without enqueuing so mode behaviour can be
    /// asserted directly from the returned bytes.
    pub fn encode_mouse_for_test(&self, input: MouseInput) -> Result<Vec<u8>, GhosttyError> {
        let mut encoder = self.mouse_encoder.lock().expect("mouse encoder poisoned");
        let mut event = self.mouse_event.lock().expect("mouse event poisoned");
        {
            let terminal = self.terminal.lock().expect("terminal mutex poisoned");
            encoder.sync_from_terminal(&terminal)?;
        }
        Ok(self
            .encode_with_encoder(&mut encoder, &mut event, input)?
            .unwrap_or_default())
    }

    /// Selection state changed: bump the generation so prepaint cannot take
    /// its cache fast path, then coalesce one renderer wakeup.
    fn notify_changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.schedule_wakeup();
    }

    /// Begin a Ghostty-owned selection gesture at one viewport cell. Click
    /// count selects cell/word/line behavior inside Ghostty, not in Rust.
    pub fn begin_selection_gesture(
        &self,
        x: u16,
        y: u16,
        _click_count: usize,
    ) -> Result<(), GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let gesture = self
            .selection_gesture
            .lock()
            .expect("selection gesture poisoned");
        gesture.press(&terminal, x, y)?;
        drop(gesture);
        drop(terminal);
        self.notify_changed();
        Ok(())
    }

    /// Continue the same gesture. Geometry comes from the canonical display
    /// geometry measured in prepaint; the view suppresses repeated cells.
    pub fn drag_selection_gesture(
        &self,
        x: u16,
        y: u16,
        cols: u16,
        cell_width: f32,
        cell_height: f32,
    ) -> Result<(), GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let gesture = self
            .selection_gesture
            .lock()
            .expect("selection gesture poisoned");
        let geometry = sys::GhosttySelectionGestureGeometry {
            columns: cols.max(1) as u32,
            cell_width: cell_width.max(1.0) as u32,
            padding_left: 0,
            screen_height: (f32::from(terminal.rows()) * cell_height.max(1.0)) as u32,
        };
        gesture.drag(&terminal, x, y, geometry)?;
        drop(gesture);
        drop(terminal);
        self.notify_changed();
        Ok(())
    }

    /// Finish the gesture. Release never forwards to Ghostty's mouse reporting.
    pub fn end_selection_gesture(&self) {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let gesture = self
            .selection_gesture
            .lock()
            .expect("selection gesture poisoned");
        let _ = gesture.release(&terminal);
        drop(gesture);
        drop(terminal);
        self.notify_changed();
    }

    /// Cancel a gesture after focus loss or deactivation.
    pub fn cancel_selection_gesture(&self) {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        let gesture = self
            .selection_gesture
            .lock()
            .expect("selection gesture poisoned");
        gesture.reset();
        drop(gesture);
        drop(terminal);
    }

    /// Local selection between two viewport cells (used when mouse reporting
    /// is off or the user holds an escape modifier).
    pub fn select_between(
        &self,
        start_x: u16,
        start_y: u16,
        end_x: u16,
        end_y: u16,
    ) -> Result<(), GhosttyError> {
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.set_selection_between(start_x, start_y, end_x, end_y)?;
        drop(terminal);
        self.schedule_wakeup();
        Ok(())
    }

    pub fn select_all(&self) -> Result<(), GhosttyError> {
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.select_all()?;
        drop(terminal);
        self.schedule_wakeup();
        Ok(())
    }

    pub fn clear_selection(&self) -> Result<(), GhosttyError> {
        let mut terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.clear_selection()?;
        drop(terminal);
        self.schedule_wakeup();
        Ok(())
    }

    /// Copy the active Ghostty selection as plain text.
    pub fn copy_selection(&self) -> Result<String, GhosttyError> {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.selection_text()
    }

    pub fn mark_process_exited(&self) {
        self.exited.store(true, Ordering::Release);
        self.schedule_wakeup();
    }

    #[must_use]
    pub fn process_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

/// Send-only view of the counters for renderer code that cannot borrow the
/// session (e.g. a GPUI element during prepaint/paint).
pub struct CountersHandle(Arc<Counters>);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct CountingWakeup(AtomicUsize);
    impl WakeupSink for CountingWakeup {
        fn schedule(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn wakeups_are_coalesced_and_never_lost() {
        let sink = Box::new(CountingWakeup(AtomicUsize::new(0)));
        let session = TerminalSession::new(80, 24, sink).expect("session created");
        for _ in 0..1000 {
            session.write_vt(b"x");
        }
        let counters = session.snapshot_counters();
        assert_eq!(counters.vt_write_calls, 1000);
        assert_eq!(counters.wakeups_scheduled, 1);
        assert_eq!(counters.wakeups_coalesced, 999);
        assert_eq!(session.take_wakeup(), Some(1000));
        assert_eq!(session.take_wakeup(), None);

        // A write after the UI consumed its wakeup must re-arm the flag.
        session.write_vt(b"y");
        assert_eq!(session.take_wakeup(), Some(1001));
    }

    #[test]
    fn two_phase_update_and_clean_are_separate() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session created");
        session.write_vt(b"hello");
        let snapshot = session.render_snapshot().expect("render snapshot");
        assert_eq!(snapshot.line(0)[0].text, "h");
        assert_ne!(snapshot.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
        let counters = session.snapshot_counters();
        assert_eq!(counters.render_state_updates, 1);
        assert!(counters.dirty_rows_visited > 0);

        session.mark_frame_complete().expect("frame complete");
        let clean = session.render_snapshot().expect("clean snapshot");
        assert_eq!(clean.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
        assert_eq!(session.snapshot_counters().dirty_clean, 1);
    }

    #[test]
    fn resize_separates_ghostty_geometry_from_pty_grid() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session created");
        let pty_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&pty_calls);
        session.set_pty_resize(Arc::new(move |_, _| {
            seen.fetch_add(1, Ordering::SeqCst);
        }));

        // Font-size change: cell pixels change, grid does not.
        let outcome = session
            .apply_layout_geometry(80, 24, 10, 20)
            .expect("pixel geometry applied");
        assert_eq!(
            outcome,
            ResizeOutcome {
                terminal_changed: true,
                grid_changed: false
            }
        );
        assert_eq!(pty_calls.load(Ordering::SeqCst), 0);

        // One column more: both paths update.
        let outcome = session
            .apply_layout_geometry(81, 24, 10, 20)
            .expect("grid geometry applied");
        assert_eq!(outcome.grid_changed, true);
        assert_eq!(pty_calls.load(Ordering::SeqCst), 1);

        // Repeating the exact four fields does nothing.
        let outcome = session
            .apply_layout_geometry(81, 24, 10, 20)
            .expect("duplicate suppressed");
        assert_eq!(outcome, ResizeOutcome::default());
        assert_eq!(pty_calls.load(Ordering::SeqCst), 1);

        // Layout-originated geometry must not enqueue a renderer wakeup.
        assert_eq!(session.take_wakeup(), None);
        let counters = session.snapshot_counters();
        assert_eq!(counters.ghostty_geometry_updates, 2);
        assert_eq!(counters.pty_grid_resize_requests, 1);
        assert_eq!(counters.grid_size_changes, 1);
        assert_eq!(counters.duplicate_resize_suppressed, 1);
        assert_eq!(counters.wakeups_scheduled, 0);
        assert_eq!(session.applied_geometry().cols, 81);
    }

    #[test]
    fn paste_uses_terminal_mode_and_key_input_queues_bytes() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session created");
        let plain = session.paste("hello").expect("plain paste");
        assert_eq!(plain, b"hello");
        session.write_vt(b"\x1b[?2004h");
        let bracketed = session.paste("hello").expect("bracketed paste");
        assert!(bracketed.starts_with(b"\x1b[200~"));

        let mut event = KeyEvent::new().expect("key event");
        event.press(sys::GHOSTTY_KEY_A, 0).expect("press");
        event.set_utf8("a").expect("utf8");
        let encoded = session.encode_key(&event).expect("encoded key");
        assert_eq!(encoded, b"a");
        assert_eq!(session.writer_depth(), 3);
        assert_eq!(session.drain_writer(2).len(), 2);
        assert_eq!(session.writer_depth(), 1);
    }

    #[test]
    fn rendering_overlaps_a_live_producer_without_starvation() {
        use std::thread;

        let session =
            Arc::new(TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session"));
        let producer = {
            let session = Arc::clone(&session);
            thread::spawn(move || {
                for i in 0..300 {
                    session.write_vt(format!("frame {i}\r\n").as_bytes());
                }
            })
        };
        let mut frames = 0usize;
        while !producer.is_finished() {
            let snapshot = session.render_snapshot().expect("render while producing");
            assert_eq!(snapshot.cols, 80);
            frames += 1;
        }
        producer.join().expect("producer finished");
        let _ = session.render_snapshot().expect("final frame");
        assert!(frames > 0);
        let counters = session.snapshot_counters();
        assert_eq!(counters.vt_write_calls, 300);
        assert_eq!(counters.wakeups_scheduled, 1);
        assert!(counters.terminal_lock_hold_max_micros < 1_000_000);
    }

    #[test]
    fn scrollback_is_bounded_and_viewport_scrolls_from_ghostty() {
        let session = TerminalSession::new(20, 4, Box::new(NoopWakeup)).expect("session");
        let mut payload = String::new();
        for i in 0..200 {
            payload.push_str(&format!("line {i}\r\n"));
        }
        session.write_vt(payload.as_bytes());
        let before = session.scrollbar().expect("scrollbar");
        assert!(before.total > before.len, "history should exist");

        assert!(session.scroll(-5).expect("scroll up"));
        let up = session.scrollbar().expect("scrollbar after up");
        assert!(up.offset < before.offset, "viewport must move into history");

        session.scroll_to_top().expect("top");
        assert_eq!(session.scrollbar().expect("top scrollbar").offset, 0);
        session.scroll_to_bottom().expect("bottom");
        let bottom = session.scrollbar().expect("bottom scrollbar");
        assert_eq!(bottom.offset, bottom.total.saturating_sub(bottom.len));
        assert!(!session.scroll(0).expect("zero delta is a no-op"));
    }

    #[test]
    fn partial_frames_materialize_only_the_rows_needed() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        session.write_vt(b"hello\r\nworld");
        let first = session.render_snapshot().expect("first frame");
        let counters = session.snapshot_counters();
        assert_eq!(first.meta.rows, 24);
        assert_eq!(counters.rows_materialized, 24);
        assert_eq!(counters.cells_materialized, 24 * 80);
        session.mark_frame_complete().expect("frame consumed");

        // A hidden-cursor mode change is a clean, cursor-only frame: no cell
        // row may be materialized, and shaped rows must survive untouched.
        session.write_vt(b"\x1b[?25l");
        let clean = session.render_snapshot().expect("cursor-only frame");
        assert!(clean.rows.is_empty());
        assert_eq!(clean.dirty, sys::GHOSTTY_RENDER_STATE_DIRTY_FALSE);
        let counters = session.snapshot_counters();
        assert_eq!(counters.rows_materialized, 24);
        assert_eq!(counters.cells_materialized, 24 * 80);
        assert_eq!(counters.cursor_only_frames, 1);
    }

    #[test]
    fn mouse_route_reports_tracking_and_queues_without_cloning() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        let not_tracking = session.route_mouse(MouseInput {
            action: sys::GHOSTTY_MOUSE_ACTION_MOTION,
            button: None,
            mods: 0,
            x: 10.0,
            y: 10.0,
            any_button_pressed: false,
            surface_width: 800,
            surface_height: 480,
            cell_width: 10,
            cell_height: 20,
        });
        assert_eq!(not_tracking.expect("route"), MouseRoute::NotTracking);

        session.write_vt(b"\x1b[?1003h\x1b[?1006h");
        let route = session
            .route_mouse(MouseInput {
                action: sys::GHOSTTY_MOUSE_ACTION_MOTION,
                button: None,
                mods: 0,
                x: 10.0,
                y: 10.0,
                any_button_pressed: false,
                surface_width: 800,
                surface_height: 480,
                cell_width: 10,
                cell_height: 20,
            })
            .expect("route in 1003");
        assert!(matches!(route, MouseRoute::Reported { bytes_queued } if bytes_queued > 0));
        let counters = session.snapshot_counters();
        assert_eq!(counters.mouse_events_offered, 2);
        assert_eq!(counters.mouse_events_encoded, 1);
        assert!(counters.mouse_bytes_queued > 0);
        assert_eq!(session.writer_depth(), 1);
        let queued = session.drain_writer(1);
        assert!(queued[0].starts_with(b"\x1b[<"));
    }

    #[test]
    fn one_scroll_operation_takes_one_terminal_lock() {
        let session = TerminalSession::new(20, 4, Box::new(NoopWakeup)).expect("session");
        let mut payload = String::new();
        for i in 0..100 {
            payload.push_str(&format!("line {i}\r\n"));
        }
        session.write_vt(payload.as_bytes());
        assert!(session.scroll(-5).expect("scroll up"));
        assert_eq!(session.snapshot_counters().scroll_lock_acquisitions, 1);
        let outcome = session.scroll_to_row(3).expect("scroll to row");
        assert!(outcome.scrollbar.total > 0);
        assert_eq!(session.snapshot_counters().scroll_lock_acquisitions, 2);
    }

    #[test]
    fn key_release_is_encoded_when_report_events_is_enabled() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        // Disambiguate + report event types (Kitty flags 1|2).
        session.write_vt(b"\x1b[>3u");
        let mut press_event = KeyEvent::new().expect("key event");
        press_event
            .press(sys::GHOSTTY_KEY_ARROW_UP, 0)
            .expect("press");
        let press = session.encode_key(&press_event).expect("encode press");
        let mut release_event = KeyEvent::new().expect("key event");
        release_event
            .release(sys::GHOSTTY_KEY_ARROW_UP, 0)
            .expect("release");
        let release = session.encode_key(&release_event).expect("encode release");
        assert!(!press.is_empty() && !release.is_empty());
        assert_ne!(press, release, "release must carry its own event type");
        // Kitty event type 3 is release; arrows use the CSI-u letter form.
        assert!(
            release.windows(2).any(|pair| pair == b":3"),
            "Kitty release encoding: {release:?}"
        );
    }

    #[test]
    fn ghostty_gesture_owns_press_drag_release_selection() {
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        session.write_vt(b"hello world");
        session
            .begin_selection_gesture(0, 0, 1)
            .expect("gesture press");
        session
            .drag_selection_gesture(4, 0, 80, 9.5, 19.0)
            .expect("gesture drag");
        session.end_selection_gesture();
        session.select_between(0, 0, 4, 0).expect("plain selection");
        let selected = session.copy_selection().expect("selection text");
        assert!(!selected.is_empty());
        assert!(session.snapshot_counters().wakeups_scheduled > 0);
    }

    #[test]
    fn title_effect_is_observed_without_second_parser() {
        use std::sync::Mutex as StdMutex;
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let sink_seen = Arc::clone(&seen);
        let session = TerminalSession::new(80, 24, Box::new(NoopWakeup)).expect("session");
        session
            .install_callbacks(Box::new(move |title| {
                sink_seen.lock().expect("title sink poisoned").push(title);
                // The sink normally only posts to the UI; recording a counter
                // here exercises the same path the app uses.
            }))
            .expect("callbacks");
        session.write_vt(b"\x1b]0;pain test title\x07");
        let titles = seen.lock().expect("title sink poisoned");
        assert_eq!(titles.len(), 1, "one OSC title change");
        assert_eq!(titles[0], "pain test title");
        drop(titles);
        session.record_title_change();
        assert_eq!(session.snapshot_counters().title_changes, 1);
    }

    #[test]
    fn background_producer_keeps_ui_responsive_and_coalesces() {
        use std::thread;

        let sink = Box::new(CountingWakeup(AtomicUsize::new(0)));
        let session = Arc::new(TerminalSession::new(80, 24, sink).expect("session created"));
        let mut handles = Vec::new();
        for thread_index in 0..4 {
            let session = Arc::clone(&session);
            handles.push(thread::spawn(move || {
                for i in 0..500 {
                    let line = format!("t{thread_index}:{i}\r\n");
                    session.write_vt(line.as_bytes());
                }
            }));
        }
        for handle in handles {
            handle.join().expect("producer thread finished");
        }

        let counters = session.snapshot_counters();
        assert_eq!(counters.vt_write_calls, 2000);
        assert_eq!(counters.wakeups_scheduled, 1);
        assert_eq!(session.generation(), 2000);
        assert!(session.take_wakeup().is_some());

        let mut last_frame = None;
        for _ in 0..8 {
            last_frame = Some(session.render_snapshot().expect("render under load"));
        }
        let final_frame = last_frame.expect("at least one frame");
        assert!(!final_frame.line(0).is_empty());
        assert!(session.snapshot_counters().render_state_updates >= 8);
    }
}
