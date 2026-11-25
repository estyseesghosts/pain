//! M1 renderer: one custom GPUI element that draws a Ghostty render snapshot.
//!
//! The element follows GPUI's `request_layout` → `prepaint` → `paint` contract.
//! Prepaint owns all terminal access, row-cache construction and shaping;
//! paint only consumes precomputed quads and shaped lines. No cell widgets,
//! scroll container, or second grid model are created.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use gpui::{
    div, fill, hsla, point, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem,
    Context, DispatchPhase, Element, ElementId, FocusHandle, Focusable, GlobalElementId, Hsla,
    InputHandler, InspectorElementId, InteractiveElement, KeyContext, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Render,
    Rgba, ScrollDelta, ScrollEvent, ShapedLine, SharedString, StrikethroughStyle, Style, TextAlign,
    TextRun, UTF16Selection, UnderlineStyle, Window,
};
use silvestris::ActiveTheme;

use crate::terminal::geometry::{
    adjust_font_size, background_runs, batch_row, selection_span, BgRun, CellMetrics, RowCache,
    TextRun as CellRun,
};
use crate::terminal::ghostty::{Cursor, KeyEvent};
use crate::terminal::input::{
    font_command, is_app_shortcut, key_for_keystroke, mouse_escape, FontCommand,
    Modifiers as InputModifiers, MouseAction, MouseButton as PtyButton,
};
use crate::terminal::selection::{
    accumulate_wheel, line_bounds, scrollbar_geometry, scrollbar_row_at, wheel_button,
    ScrollbarGeometry, SelectionGesture,
};
use crate::terminal::session::{CountersHandle, TerminalSession};

/// A shaped row kept between frames. Clean rows are reused verbatim.
#[derive(Clone)]
struct CachedRow {
    quads: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
}

#[derive(Default)]
struct FrameCache {
    generation: u64,
    cell_metrics: Option<(f32, f32)>,
    origin: (f32, f32),
    rows: Vec<CachedRow>,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
    background: Hsla,
    row_cache: RowCache,
}

/// Geometry captured in prepaint so input handlers can map window points to
/// terminal cells without re-measuring the font on every pointer event.
#[derive(Clone, Copy, Default)]
struct InputGeometry {
    origin: (f32, f32),
    cell_width: f32,
    line_height: f32,
    screen_width: u16,
    screen_height: u16,
}

/// Application/domain view owning one session. Silvestris owns the window
/// chrome; this view is the terminal surface inside `WindowFrame::child`.
pub struct TerminalView {
    session: Arc<TerminalSession>,
    counters: Arc<CountersHandle>,
    focus: FocusHandle,
    /// Font size in logical pixels; overwritten by Ctrl+= / Ctrl+- actions.
    font_size: f32,
    cache: RefCell<FrameCache>,
    geometry: RefCell<InputGeometry>,
    selection_anchor: RefCell<Option<(u16, u16)>>,
    /// Fractional wheel rows carried between trackpad events.
    scroll_remainder: f64,
    /// Ghostty-derived scrollbar thumb, recomputed once in prepaint.
    scrollbar: RefCell<Option<ScrollbarGeometry>>,
    scrollbar_drag: bool,
    /// Which pointer buttons are held, as bits from `PtyButton::bit`. Ghostty
    /// needs this to keep reporting drags that leave the terminal surface.
    held_mouse: Cell<u8>,
}

impl TerminalView {
    #[must_use]
    pub fn new(session: Arc<TerminalSession>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let counters = session.counters();
        let focus = cx.focus_handle();
        let font_size = cx.theme().type_.size_mono;
        window.focus(&focus, cx);
        Self {
            session,
            counters,
            focus,
            font_size,
            cache: RefCell::new(FrameCache::default()),
            geometry: RefCell::new(InputGeometry::default()),
            selection_anchor: RefCell::new(None),
            scroll_remainder: 0.0,
            scrollbar: RefCell::new(None),
            scrollbar_drag: false,
            held_mouse: Cell::new(0),
        }
    }

    #[must_use]
    pub fn session(&self) -> &Arc<TerminalSession> {
        &self.session
    }

    /// Focus handle used by the Silvestris responder chain to gate app
    /// commands while the terminal canvas owns the keyboard.
    #[must_use]
    pub fn focus(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Change the renderer's font size. `cx.notify()` schedules one repaint;
    /// the metrics/row cache are invalidated in prepaint when they change.
    pub fn adjust_font(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.set_font_size(adjust_font_size(self.font_size, delta), cx);
    }

    pub fn set_font_size(&mut self, size: f32, cx: &mut Context<Self>) {
        let size = adjust_font_size(size, 0.0);
        if (size - self.font_size).abs() > f32::EPSILON {
            self.font_size = size;
            cx.notify();
        }
    }

    #[must_use]
    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    fn cell_at(&self, position: Point<Pixels>) -> (u16, u16) {
        let geometry = *self.geometry.borrow();
        let x = ((f32::from(position.x) - geometry.origin.0) / geometry.cell_width.max(1.0))
            .max(0.0) as u16;
        let y = ((f32::from(position.y) - geometry.origin.1) / geometry.line_height.max(1.0))
            .max(0.0) as u16;
        (x, y)
    }

    /// Control keys are encoded by Ghostty against the terminal's current
    /// modes. Plain printable text propagates to GPUI's native text handler,
    /// which owns it on every platform; font shortcuts are consumed here.
    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let name = event.keystroke.key.as_str();
        // Handle the font commands here as well as via global actions: a
        // terminal-focused view consumes most keystrokes, so relying only on
        // the global action made Ctrl+=/- unreliable under WSLg.
        if let Some(command) = font_command(
            name,
            event.keystroke.key_char.as_deref(),
            event.keystroke.modifiers.control,
        ) {
            let step = crate::terminal::geometry::FONT_SIZE_STEP;
            match command {
                FontCommand::Increase => self.adjust_font(step, cx),
                FontCommand::Decrease => self.adjust_font(-step, cx),
                FontCommand::Reset => self.set_font_size(cx.theme().type_.size_mono, cx),
            }
            cx.stop_propagation();
            return;
        }
        let m = event.keystroke.modifiers;
        let mods = InputModifiers::new(m.shift, m.control, m.alt, m.platform);
        // GPUI dispatches these bindings before this listener. Let the action
        // own the key entirely; encoding it too would send a stray sequence
        // to the PTY after a clipboard copy or paste.
        if is_app_shortcut(name, mods) {
            return;
        }
        // The pure policy decides whether this key belongs to Ghostty's
        // encoder or to the platform's native text handler, and always keeps
        // the committed text for printable keys such as space.
        let Some(encoded) = key_for_keystroke(
            name,
            event.keystroke.key_char.as_deref(),
            mods,
            event.prefer_character_input,
            event.is_held,
        ) else {
            return;
        };
        if let Ok(mut key_event) = KeyEvent::new() {
            if encoded.repeat {
                let _ = key_event.repeat(encoded.key, encoded.mods.ghostty_bits());
            } else {
                let _ = key_event.press(encoded.key, encoded.mods.ghostty_bits());
            }
            if let Some(text) = &encoded.text {
                let _ = key_event.set_utf8(text);
            }
            let _ = key_event.set_unshifted_codepoint(encoded.unshifted_codepoint);
            let _ = self.session.encode_key(&key_event);
        }
        cx.stop_propagation();
    }

    /// Wheel/trackpad scrolling. When a TUI reports mouse tracking and no
    /// escape modifier is held, wheel events are encoded for the child.
    /// Otherwise they move Ghostty's viewport through its own scroll API.
    fn on_scroll(&mut self, event: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let up_pixels = match event.delta {
            ScrollDelta::Pixels(delta) => f64::from(delta.y),
            ScrollDelta::Lines(delta) => {
                f64::from(delta.y) * f64::from(self.geometry.borrow().line_height)
            }
        };
        if up_pixels == 0.0 {
            return;
        }
        let line_height = self.geometry.borrow().line_height;
        let (rows, remainder) = accumulate_wheel(self.scroll_remainder, up_pixels, line_height);
        self.scroll_remainder = remainder;
        if rows == 0 {
            return;
        }
        let tracking = self.session.mouse_tracking_enabled().unwrap_or(false);
        if tracking && !event.modifiers.platform {
            // One terminal mouse wheel report per whole row.
            let button = wheel_button(rows < 0);
            let (px, py, w, h, mods) = {
                let (x, _y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
                let _ = x;
                (px, py, w, h, mods)
            };
            for _ in 0..rows.unsigned_abs() {
                let _ = self.session.encode_mouse(
                    MouseAction::Press.ghostty(),
                    Some(button),
                    mods.ghostty_bits(),
                    px,
                    py,
                    self.held_mouse.get() != 0,
                    u32::from(w),
                    u32::from(h),
                    self.geometry.borrow().cell_width.max(1.0) as u32,
                    self.geometry.borrow().line_height.max(1.0) as u32,
                );
            }
        } else {
            // `accumulate_wheel` already returns Ghostty's delta convention:
            // negative rows move up into history.
            let _ = self.session.scroll(rows);
        }
        cx.stop_propagation();
    }

    /// Apply a click gesture to the visible row. Word/line bounds are derived
    /// from Ghostty's rendered cells, so selection stays Ghostty-owned.
    fn select_gesture(&self, x: u16, y: u16, click_count: usize) {
        let Ok(snapshot) = self.session.render_snapshot() else {
            return;
        };
        let Some(cells) = snapshot.lines.get(usize::from(y)) else {
            return;
        };
        let (start, end) = match SelectionGesture::from_click_count(click_count) {
            SelectionGesture::Character => (usize::from(x), usize::from(x)),
            SelectionGesture::Word => {
                let (start, end) = crate::terminal::selection::word_bounds(cells, usize::from(x));
                (start, end)
            }
            SelectionGesture::Line => line_bounds(cells),
        };
        let _ = self.session.select_between(start as u16, y, end as u16, y);
    }

    fn mouse_event(
        &self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) -> (u16, u16, u16, u16, f32, f32, InputModifiers) {
        let geometry = *self.geometry.borrow();
        let (x, y) = self.cell_at(position);
        let mods = InputModifiers::new(
            modifiers.shift,
            modifiers.control,
            modifiers.alt,
            modifiers.platform,
        );
        (
            x,
            y,
            geometry.screen_width,
            geometry.screen_height,
            f32::from(position.x) - geometry.origin.0,
            f32::from(position.y) - geometry.origin.1,
            mods,
        )
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let (x, y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        if event.button == MouseButton::Left {
            if let Some(bar) = *self.scrollbar.borrow() {
                let local_x = f32::from(event.position.x) - bar.track_x;
                if (0.0..=bar.track_width).contains(&local_x) {
                    self.scrollbar_drag = true;
                    let bar_state = self.session.scrollbar();
                    if let Ok(bar_state) = bar_state {
                        let row = scrollbar_row_at(
                            &bar,
                            f32::from(event.position.y),
                            bar_state.total,
                            bar_state.len,
                        );
                        let _ = self.session.scroll_to_row(row);
                    }
                    cx.stop_propagation();
                    return;
                }
            }
        }
        let Some(button) = pty_button(event.button) else {
            return;
        };
        self.held_mouse.set(self.held_mouse.get() | button.bit());
        let held = self.held_mouse.get() != 0;
        let tracking = self.session.mouse_tracking_enabled().unwrap_or(false);
        if tracking && !mouse_escape(mods) {
            let _ = self.session.encode_mouse(
                MouseAction::Press.ghostty(),
                button.ghostty(),
                mods.ghostty_bits(),
                px,
                py,
                held,
                u32::from(w),
                u32::from(h),
                self.geometry.borrow().cell_width.max(1.0) as u32,
                self.geometry.borrow().line_height.max(1.0) as u32,
            );
            cx.stop_propagation();
        } else if button == PtyButton::Left {
            // Only the left button starts a local selection. Right/middle with
            // reporting off belong to the platform (context menu), not Ghostty.
            *self.selection_anchor.borrow_mut() = Some((x, y));
            self.select_gesture(x, y, event.click_count);
            cx.stop_propagation();
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.scrollbar_drag {
            if let (Some(bar), Ok(state)) = (*self.scrollbar.borrow(), self.session.scrollbar()) {
                let row =
                    scrollbar_row_at(&bar, f32::from(event.position.y), state.total, state.len);
                let _ = self.session.scroll_to_row(row);
            }
            cx.stop_propagation();
            return;
        }
        let (x, y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        let held = self.held_mouse.get() != 0;
        let tracking = self.session.mouse_tracking_enabled().unwrap_or(false);
        if tracking && !mouse_escape(mods) {
            // Ghostty decides whether this motion is reported: mode 1000 drops
            // all motion, 1002 requires a held button, 1003 reports hover too.
            // A `None` button is the correct "motion, nothing held" encoding.
            let button = event
                .pressed_button
                .and_then(pty_button)
                .and_then(PtyButton::ghostty);
            let _ = self.session.encode_mouse(
                MouseAction::Motion.ghostty(),
                button,
                mods.ghostty_bits(),
                px,
                py,
                held,
                u32::from(w),
                u32::from(h),
                self.geometry.borrow().cell_width.max(1.0) as u32,
                self.geometry.borrow().line_height.max(1.0) as u32,
            );
            cx.stop_propagation();
        } else if let Some((start_x, start_y)) = *self.selection_anchor.borrow() {
            let _ = self.session.select_between(start_x, start_y, x, y);
            cx.stop_propagation();
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.scrollbar_drag {
            self.scrollbar_drag = false;
            cx.stop_propagation();
            return;
        }
        let (_x, _y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        let Some(button) = pty_button(event.button) else {
            return;
        };
        self.held_mouse.set(self.held_mouse.get() & !button.bit());
        let held = self.held_mouse.get() != 0;
        let tracking = self.session.mouse_tracking_enabled().unwrap_or(false);
        if tracking && !mouse_escape(mods) {
            let _ = self.session.encode_mouse(
                MouseAction::Release.ghostty(),
                button.ghostty(),
                mods.ghostty_bits(),
                px,
                py,
                held,
                u32::from(w),
                u32::from(h),
                self.geometry.borrow().cell_width.max(1.0) as u32,
                self.geometry.borrow().line_height.max(1.0) as u32,
            );
            cx.stop_propagation();
        } else if button == PtyButton::Left {
            *self.selection_anchor.borrow_mut() = None;
            cx.stop_propagation();
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context(KeyContext::new_with_defaults(cx.platform_info().os))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .child(TerminalElement { view: cx.entity() })
    }
}

fn pty_button(button: MouseButton) -> Option<PtyButton> {
    match button {
        MouseButton::Left => Some(PtyButton::Left),
        MouseButton::Right => Some(PtyButton::Right),
        MouseButton::Middle => Some(PtyButton::Middle),
        MouseButton::Navigate(_) => None,
    }
}

struct TerminalElement {
    view: gpui::Entity<TerminalView>,
}

/// Everything paint needs, precomputed in prepaint.
struct FrameState {
    rows: Vec<CachedRow>,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
    background: Hsla,
    scrollbar: Option<ScrollbarGeometry>,
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = FrameState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let view = self.view.read(cx);
        let session = Arc::clone(&view.session);
        let counters = Arc::clone(&view.counters);
        let start = std::time::Instant::now();

        let theme = cx.theme().clone();
        let font_family: SharedString = theme.type_.mono.into();
        // The theme supplies the family and initial size; Ctrl+=/- changes
        // only the size override held by the view.
        let font_size = px(view.font_size);
        let mono_font = gpui::font(font_family);
        let mut probe_font = mono_font.clone();
        probe_font.weight = gpui::FontWeight::NORMAL;
        let probe_run = TextRun {
            len: 1,
            font: probe_font,
            color: hsla(0., 0., 0., 1.),
            ..Default::default()
        };
        let probe = window
            .text_system()
            .shape_line("M".into(), font_size, &[probe_run], None);
        let cell_width = f32::from(probe.width()).max(1.0);
        let line_height = (f32::from(probe.ascent + probe.descent) * 1.25)
            .max(f32::from(font_size))
            .max(1.0);
        let metrics = CellMetrics::new(cell_width, line_height);
        // Keep input mapping in sync even on a cache-hit frame: dragging the
        // window changes `bounds` without changing any cached rows.
        *view.geometry.borrow_mut() = InputGeometry {
            origin: (f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
            cell_width,
            line_height,
            screen_width: (f32::from(bounds.size.width).max(0.0) as u32).min(u16::MAX as u32)
                as u16,
            screen_height: (f32::from(bounds.size.height).max(0.0) as u32).min(u16::MAX as u32)
                as u16,
        };
        let (cols, rows) =
            metrics.grid_for(f32::from(bounds.size.width), f32::from(bounds.size.height));

        // The scrollbar thumb is a view over Ghostty's row counts, never a GPUI
        // ScrollHandle or a second history model.
        let bar = session.scrollbar().ok().and_then(|state| {
            scrollbar_geometry(
                state.total,
                state.offset,
                state.len,
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            )
        });
        *view.scrollbar.borrow_mut() = bar;

        let origin_key = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        let metric_key = (cell_width, line_height);
        let wake = session.take_wakeup();
        let generation = session.generation();

        {
            let cache = view.cache.borrow();
            // A pixel-only window resize leaves the cached rows valid: their
            // quads and text origins depend on the content origin and cell
            // metrics, not on the window's width/height. Rebuilding here was
            // the source of jank during a drag.
            if wake.is_none()
                && cache.generation == generation
                && cache.origin == origin_key
                && cache.cell_metrics == Some(metric_key)
                && cache.rows.len() == usize::from(rows)
            {
                counters.prepaint_micros(
                    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
                );
                return FrameState {
                    rows: cache.rows.clone(),
                    cursor: cache.cursor.clone(),
                    line_height: cache.line_height,
                    background: cache.background,
                    scrollbar: bar,
                };
            }
        }

        // A grid change must reach Ghostty before reading render state.
        if !session
            .resize(cols, rows, cell_width as u32, line_height as u32)
            .expect("terminal grid resize")
        {
            // Pixel-only change: rows/cols are unchanged, reuse the cached
            // rows while recomputing only the origin below.
        }
        let snapshot = session.render_snapshot().expect("render snapshot");
        let (previous, rebuild, row_cache): (Vec<CachedRow>, Vec<usize>, RowCache) = {
            let mut cache = view.cache.borrow_mut();
            let listed =
                cache
                    .row_cache
                    .rows_to_rebuild(generation, snapshot.dirty, &snapshot.dirty_rows);
            let invalid = cache.origin != origin_key
                || cache.cell_metrics != Some(metric_key)
                || cache.rows.len() != usize::from(rows);
            let rebuild = if invalid {
                cache.row_cache = RowCache::new(usize::from(rows));
                let _ = cache
                    .row_cache
                    .rows_to_rebuild(generation, snapshot.dirty, &[]);
                (0..usize::from(rows)).collect()
            } else {
                listed
            };
            (cache.rows.clone(), rebuild, cache.row_cache.clone())
        };
        let rebuild_set: std::collections::HashSet<usize> = rebuild.into_iter().collect();
        let palette = &snapshot.colors.palette;
        let default_bg = rgb(snapshot.colors.background);
        let default_fg = rgb(snapshot.colors.foreground);
        let origin = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));

        let mut built: Vec<CachedRow> = Vec::with_capacity(usize::from(rows));
        for y in 0..usize::from(rows) {
            if !rebuild_set.contains(&y) {
                if let Some(row) = previous.get(y) {
                    built.push(row.clone());
                    counters.row_cache_hits(1);
                    continue;
                }
            }
            let cells = snapshot.lines.get(y).map(Vec::as_slice).unwrap_or(&[]);
            let mut quads = Vec::new();
            let mut lines = Vec::new();

            for run_bg in background_runs(cells, default_spec(snapshot.colors.background), palette)
            {
                quads.push(bg_quad(&metrics, origin, y, run_bg));
                counters.background_quads(1);
            }
            if let Some((start_x, cells_len)) =
                selection_span(snapshot.selections.get(y).copied().flatten())
            {
                let (x, top, w, h) = metrics.span_bounds(origin, start_x, y as u16, cells_len);
                quads.push(fill(
                    Bounds::new(point(px(x), px(top)), size(px(w), px(h))),
                    rgba(0x446666cc),
                ));
                counters.background_quads(1);
            }

            for run in batch_row(cells) {
                let text: SharedString = run.text.clone().into();
                let shape = shape_run(
                    &run, text, &mono_font, font_size, &metrics, default_fg, window, &counters,
                );
                let (x, top, _, _) = metrics.span_bounds(origin, run.start_col, y as u16, 1);
                lines.push((point(px(x), px(top)), shape));
                counters.text_batches(1);
            }
            built.push(CachedRow { quads, lines });
            counters.rows_rebuilt(1);
        }

        let cursor = if snapshot.cursor.visible && snapshot.cursor.has_value {
            cursor_quad(&metrics, origin, &snapshot.cursor, default_fg)
        } else {
            None
        };

        *view.cache.borrow_mut() = FrameCache {
            generation,
            cell_metrics: Some(metric_key),
            origin: origin_key,
            rows: built.clone(),
            cursor: cursor.clone(),
            line_height: px(line_height),
            background: default_bg,
            row_cache,
        };
        session.mark_frame_complete().expect("frame consumed");
        counters.prepaint_micros(u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX));
        FrameState {
            rows: built,
            cursor,
            line_height: px(line_height),
            background: default_bg,
            scrollbar: bar,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let start = std::time::Instant::now();
        window.paint_quad(fill(bounds, prepaint.background));
        for row in &prepaint.rows {
            for quad in &row.quads {
                window.paint_quad(quad.clone());
            }
            for (origin, line) in &row.lines {
                line.paint(
                    *origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .expect("paint shaped terminal line");
            }
        }
        if let Some(cursor) = &prepaint.cursor {
            window.paint_quad(cursor.clone());
        }
        if let Some(bar) = &prepaint.scrollbar {
            // Track and thumb stay a thin view over Ghostty's viewport state.
            window.paint_quad(fill(
                Bounds::new(
                    point(px(bar.track_x), px(bar.track_y)),
                    size(px(bar.track_width), px(bar.track_height)),
                ),
                hsla(0.0, 0.0, 1.0, 0.08),
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(px(bar.track_x), px(bar.thumb_y)),
                    size(px(bar.track_width), px(bar.thumb_height)),
                ),
                hsla(0.0, 0.0, 0.6, 0.45),
            ));
        }
        // GPUI delivers element mouse events only while the pointer is over
        // the element. Register move/up on the window instead so a drag keeps
        // reporting after it leaves the surface, until the button is released.
        let mouse_view = self.view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            let focused = mouse_view.read(cx).focus.is_focused(window);
            if phase != DispatchPhase::Bubble || !focused {
                return;
            }
            let _ = mouse_view.update(cx, |view, cx| view.on_mouse_move(event, window, cx));
        });
        let mouse_view = self.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            let focused = mouse_view.read(cx).focus.is_focused(window);
            if phase != DispatchPhase::Bubble || !focused {
                return;
            }
            let _ = mouse_view.update(cx, |view, cx| view.on_mouse_up(event, window, cx));
        });
        let view = self.view.read(cx);
        window.handle_input(
            &view.focus,
            TerminalInputHandler {
                session: Arc::clone(&view.session),
            },
            cx,
        );
        view.counters
            .paint_micros(u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
}

/// Native text/IME bridge. Committed text goes straight to the PTY writer;
/// platform-initiated paste goes through Ghostty's bracketed-paste policy.
struct TerminalInputHandler {
    session: Arc<TerminalSession>,
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        // A stable zero-length range is enough for IME candidate placement:
        // Ghostty owns the real document and selection state.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.session.write_input(text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.session.write_input(new_text);
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut App) {}

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, _cx: &mut App) {
        if let Some(text) = item.text() {
            let _ = self.session.paste(&text);
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        Some(0)
    }
}

fn default_spec(rgb: (u8, u8, u8)) -> crate::terminal::ghostty::ColorSpec {
    crate::terminal::ghostty::ColorSpec::Rgb(rgb.0, rgb.1, rgb.2)
}

fn rgb(c: (u8, u8, u8)) -> Hsla {
    Rgba {
        r: f32::from(c.0) / 255.0,
        g: f32::from(c.1) / 255.0,
        b: f32::from(c.2) / 255.0,
        a: 1.0,
    }
    .into()
}

fn bg_quad(metrics: &CellMetrics, origin: (f32, f32), row: usize, run: BgRun) -> PaintQuad {
    let (x, y, w, h) = metrics.span_bounds(origin, run.start_col, row as u16, run.cells);
    fill(
        Bounds::new(point(px(x), px(y)), size(px(w), px(h))),
        rgb(run.color),
    )
}

fn shape_run(
    run: &CellRun,
    text: SharedString,
    base_font: &gpui::Font,
    font_size: Pixels,
    metrics: &CellMetrics,
    default_fg: Hsla,
    window: &mut Window,
    counters: &CountersHandle,
) -> ShapedLine {
    let mut font = base_font.clone();
    if run.style.bold {
        font = font.bold();
    }
    if run.style.italic {
        font = font.italic();
    }
    let color = run.style.foreground.map(rgb).unwrap_or(default_fg);
    let underline = if run.style.underline == 0 {
        None
    } else {
        Some(UnderlineStyle {
            color: Some(color),
            thickness: px(1.0),
            wavy: run.style.underline == 3,
        })
    };
    let strikethrough = if run.style.strikethrough {
        Some(StrikethroughStyle {
            color: Some(color),
            thickness: px(1.0),
        })
    } else {
        None
    };
    let text_run = TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline,
        strikethrough,
    };
    // GPUI's `force_width` is the advance assigned to EACH glyph cell, not
    // the total run width. Passing the run width spreads glyphs apart (Zed's
    // pinned terminal element also passes one cell width here).
    let shaped = window.text_system().shape_line(
        text.clone(),
        font_size,
        &[text_run],
        Some(px(metrics.cell_width)),
    );
    counters.shape_line(text.len());
    shaped
}

fn cursor_quad(
    metrics: &CellMetrics,
    origin: (f32, f32),
    cursor: &Cursor,
    fallback: Hsla,
) -> Option<PaintQuad> {
    let (x, y, w, h) = metrics.span_bounds(origin, cursor.x, cursor.y, 1);
    let color = if cursor.password_input {
        hsla(0., 0., 0., 0.)
    } else {
        fallback
    };
    let block = matches!(cursor.visual_style, 1 | 3);
    let rect = if block {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    } else if cursor.visual_style == 2 {
        Bounds::new(point(px(x), px(y + h - 2.0)), size(px(w), px(2.0)))
    } else {
        Bounds::new(point(px(x), px(y)), size(px(2.0), px(h)))
    };
    Some(fill(rect, color))
}
