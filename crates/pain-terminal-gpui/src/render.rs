//! M1 renderer: one custom GPUI element that draws a Ghostty render snapshot.
//!
//! The element follows GPUI's `request_layout` → `prepaint` → `paint` contract.
//! Prepaint owns all terminal access, row-cache construction and shaping;
//! paint only consumes precomputed quads and shaped lines. No cell widgets,
//! scroll container, or second grid model are created.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::images::{ImageCache, ImageDraw};
use gpui::{
    div, fill, hsla, point, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem,
    Context, Corners, DispatchPhase, Element, ElementId, FocusHandle, Focusable, GlobalElementId,
    Hsla, InputHandler, InspectorElementId, InteractiveElement, KeyContext, KeyDownEvent,
    KeyUpEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, Rgba, ScrollDelta, ScrollEvent, ShapedLine, SharedString,
    StrikethroughStyle, Style, Subscription, TextAlign, TextRun, UTF16Selection, UnderlineStyle,
    Window,
};
use pain_terminal_core::geometry::{
    adjust_font_size, background_runs, batch_row, line_height_for, selection_span, BgRun,
    CellMetrics, InputGeometry, RowCache, TerminalGeometry, TextRun as CellRun, FONT_SIZE_STEP,
};
use pain_terminal_core::ghostty::{Cursor, KeyEvent};
use pain_terminal_core::input::{
    key_for_keystroke, key_release_for_keystroke, mouse_escape, Modifiers as InputModifiers,
    MouseAction, MouseButton as PtyButton,
};
use pain_terminal_core::selection::{
    accumulate_wheel, scrollbar_geometry, scrollbar_row_at, wheel_button, ScrollbarGeometry,
};
use pain_terminal_core::session::{CountersHandle, MouseInput, MouseRoute, TerminalSession};

// Intrinsic terminal actions. They are defined with the view, never bound to
// keys here: the host owns shortcut policy and binds whatever it wants.
gpui::actions!(
    terminal,
    [
        IncreaseFont,
        DecreaseFont,
        ResetFont,
        CopySelection,
        PasteClipboard,
        SelectAllTerminal
    ]
);

/// Explicit font configuration supplied by the host. The renderer does not
/// import a theme or read any theme global, so multiple views may use
/// different families and sizes in the same application.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalFont {
    pub family: SharedString,
    pub size: f32,
}

impl TerminalFont {
    #[must_use]
    pub fn new(family: impl Into<SharedString>, size: f32) -> Self {
        Self {
            family: family.into(),
            size,
        }
    }
}

/// Host-owned predicate for keys it claims before terminal input. Windows
/// needs an early raw-keystroke interceptor for `prefer_character_input`; the
/// view asks the same host policy so the two never disagree.
pub type ShortcutFilter = Arc<dyn Fn(&str, Option<&str>, InputModifiers) -> bool + Send + Sync>;

/// Ghostty exposes whether a cursor blinks but not a period; 530 ms matches
/// conventional terminal timing without a continuous frame loop.
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Unique GPUI element identity per view. A fixed id would make two terminal
/// panes collide in the element tree and break focus hit-testing.
static NEXT_VIEW_ID: AtomicUsize = AtomicUsize::new(0);

/// A shaped row kept between frames. Clean rows are shared, never cloned.
#[derive(Default)]
struct CachedRow {
    quads: Vec<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
}

#[derive(Default)]
struct FrameCache {
    generation: u64,
    /// The exact geometry this frame was built for. The grid is stored
    /// explicitly and never inferred from `rows.len()`.
    geometry: InputGeometry,
    cols: u16,
    rows: u16,
    /// Canonical owner of shaped rows. Prepaint replaces only dirty entries;
    /// prepaint/paint share this handle through `Arc` rather than deep copies.
    shaped_rows: Arc<Vec<Arc<CachedRow>>>,
    image_below_background: Arc<Vec<ImageDraw>>,
    image_below_text: Arc<Vec<ImageDraw>>,
    image_above_text: Arc<Vec<ImageDraw>>,
    preedit_text: String,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
    background: Hsla,
    row_cache: RowCache,
}

/// Per-view error report. The renderer never logs by itself; the host decides
/// presentation. One message per view is enough to keep a broken terminal from
/// flooding logs on every frame.
pub type TerminalErrorSink = Arc<dyn Fn(String) + Send + Sync>;

/// Host-owned grid-change notification. The renderer reports the new grid and
/// device-pixel cell size; it never names or imports a PTY.
pub type GridChangeSink = Arc<dyn Fn(u16, u16, u32, u32) + Send + Sync>;

/// Local IME composition. Ghostty owns the terminal document; this is only
/// the uncommitted reading used for the candidate window and never a PTY write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PreeditState {
    text: String,
    /// UTF-16 selection inside `text`, as reported by the platform.
    selected: Option<(usize, usize)>,
}

/// Lightweight cursor blinking state, separate from Ghostty's semantic
/// `cursor.blinking` mode flag. Epochs make stale timers harmless.
#[derive(Default)]
struct CursorBlink {
    on: bool,
    epoch: u64,
    /// Mirrors Ghostty's semantic flag from the latest snapshot.
    blinking: bool,
}

impl CursorBlink {
    fn restart(&mut self) -> u64 {
        self.on = true;
        self.epoch = self.epoch.wrapping_add(1);
        self.epoch
    }

    fn stop(&mut self) {
        self.on = true;
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn tick(&mut self, epoch: u64) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.on = !self.on;
        true
    }
}

/// Epoch-based Kitty animation clock. It is deliberately not a frame loop: one
/// timer exists only while `next_ms` names a running animation.
#[derive(Default)]
struct KittyAnimation {
    epoch: u64,
    /// True while a timer is outstanding.
    scheduled: bool,
    /// Set by a current-epoch timer; forces one uncached prepaint.
    pending: bool,
    next_ms: Option<u64>,
}

impl KittyAnimation {
    /// Clear and return the pending flag for this prepaint.
    fn take_pending(&mut self) -> bool {
        std::mem::take(&mut self.pending)
    }

    /// Invalidate any stale timer (stopped animation or a superseded deadline).
    fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.scheduled = false;
        self.pending = false;
        self.next_ms = None;
    }
}

/// Explicit pointer capture. Scrollbar dragging is a UI-domain gesture and
/// must never be represented by a fake terminal mouse-button bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerCapture {
    None,
    Scrollbar,
}

/// Application/domain view owning one session. Silvestris owns the window
/// chrome; this view is the terminal surface inside `WindowFrame::child`.
pub struct TerminalView {
    view_id: usize,
    session: Arc<TerminalSession>,
    counters: Arc<CountersHandle>,
    focus: FocusHandle,
    /// Host-supplied monospace family. Never read from a theme global.
    font_family: SharedString,
    /// Initial size from the host; `reset_font` returns to this value.
    default_font_size: f32,
    /// Font size in logical pixels; overwritten by Ctrl+= / Ctrl+- actions.
    font_size: f32,
    /// Host-owned shortcut policy. The view asks this so its ownership check
    /// agrees with the host's early keystroke interceptor.
    shortcut_filter: RefCell<ShortcutFilter>,
    cache: RefCell<FrameCache>,
    geometry: RefCell<InputGeometry>,
    selection_anchor: RefCell<Option<(u16, u16)>>,
    /// Fractional wheel rows carried between trackpad events.
    scroll_remainder: f64,
    /// Ghostty-derived scrollbar thumb and the state behind it, both
    /// recomputed once in prepaint or from a scroll operation's result.
    scrollbar: RefCell<Option<ScrollbarGeometry>>,
    scroll_state: RefCell<Option<pain_terminal_core::ghostty::ScrollbarState>>,
    /// Explicit capture state for the scrollbar thumb.
    capture: PointerCapture,
    /// Kept alive so focus loss / deactivation can always cancel a drag.
    subscriptions: Vec<Subscription>,
    /// Which pointer buttons are held, as bits from `PtyButton::bit`. Ghostty
    /// needs this to keep reporting drags that leave the terminal surface.
    held_mouse: Cell<u8>,
    /// Keys whose press was encoded into Ghostty. Only these may produce a
    /// matching release; app/IME/text-owned keys never do.
    held_keys: RefCell<Vec<(String, pain_terminal_core::input::EncodedKey)>>,
    /// Last cell visited by a local selection drag. Ghostty's gesture API must
    /// not be fed a second drag event for the same cell.
    last_drag_cell: Cell<Option<(u16, u16)>>,
    /// Temporal cursor visibility; never changes Ghostty's terminal modes.
    blink: RefCell<CursorBlink>,
    /// Bounded GPUI texture cache keyed by protocol/image/generation.
    images: RefCell<ImageCache>,
    /// Demand-driven Kitty animation timer state.
    kitty: RefCell<KittyAnimation>,
    /// Uncommitted native text composition, local to this view.
    preedit: RefCell<PreeditState>,
    /// Cursor cell from the latest snapshot, used to place IME candidates.
    cursor_cell: Cell<(u16, u16)>,
    /// Host-owned report path for pane-local recoverable failures.
    error_sink: RefCell<TerminalErrorSink>,
    error_reported: Cell<bool>,
    /// Host-owned PTY bridge, installed by the application that owns both.
    grid_change: RefCell<Option<GridChangeSink>>,
}

impl TerminalView {
    /// Construct a terminal surface. Font configuration is explicit and focus
    /// is left to the host: creating one view never steals focus from another.
    #[must_use]
    pub fn new(
        session: Arc<TerminalSession>,
        font: TerminalFont,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let counters = session.counters();
        let focus = cx.focus_handle();
        let default_font_size = font.size;
        let mut view = Self {
            view_id: NEXT_VIEW_ID.fetch_add(1, Ordering::Relaxed),
            session,
            counters,
            focus,
            font_family: font.family,
            default_font_size,
            font_size: default_font_size,
            shortcut_filter: RefCell::new(Arc::new(|_, _, _| false)),
            cache: RefCell::new(FrameCache::default()),
            geometry: RefCell::new(InputGeometry::default()),
            selection_anchor: RefCell::new(None),
            scroll_remainder: 0.0,
            scrollbar: RefCell::new(None),
            scroll_state: RefCell::new(None),
            capture: PointerCapture::None,
            subscriptions: Vec::new(),
            held_mouse: Cell::new(0),
            held_keys: RefCell::new(Vec::new()),
            last_drag_cell: Cell::new(None),
            blink: RefCell::new(CursorBlink::default()),
            images: RefCell::new(ImageCache::default()),
            kitty: RefCell::new(KittyAnimation::default()),
            preedit: RefCell::new(PreeditState::default()),
            cursor_cell: Cell::new((0, 0)),
            // The host installs reporting. With no sink, failures stay
            // pane-local and silent rather than dragging in a logger.
            error_sink: RefCell::new(Arc::new(|_| {})),
            error_reported: Cell::new(false),
            grid_change: RefCell::new(None),
        };
        // A lost OS release must never strand a scrollbar drag or a held
        // terminal button. Both focus loss and window deactivation cancel it.
        view.subscriptions
            .push(cx.on_focus_lost(window, |this, _, cx| {
                this.cancel_pointer_capture(cx);
            }));
        view.subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                if !window.is_window_active() {
                    this.cancel_pointer_capture(cx);
                    this.stop_blink(cx);
                } else if this.focus.is_focused(window) {
                    this.restart_blink(window, cx);
                }
            }));
        view.subscriptions
            .push(cx.on_focus_in(&view.focus, window, |this, window, cx| {
                this.restart_blink(window, cx);
            }));
        view
    }

    /// Install the bridge from terminal grid changes to the host's transport.
    /// The renderer stays unaware of PTY types and shutdown rules.
    pub fn set_grid_change_sink(&mut self, sink: GridChangeSink) {
        *self.grid_change.borrow_mut() = Some(sink);
    }

    /// Replace the default reporter. The host can route this to its own log,
    /// status area or diagnostics without the renderer knowing either.
    pub fn set_error_sink(&mut self, sink: TerminalErrorSink) {
        *self.error_sink.borrow_mut() = sink;
        self.error_reported.set(false);
    }

    /// Report one recoverable failure for this view. A second failure is
    /// silently ignored for the lifetime of the view to avoid per-frame chatter;
    /// state remains pane-local so other views are unaffected.
    fn report_error_once(&self, message: impl fmt::Display) {
        if !self.error_reported.replace(true) {
            (self.error_sink.borrow())(format!("{message}"));
        }
    }

    /// Keep the last successfully captured frame. If none exists yet, show the
    /// ordinary empty terminal surface rather than inventing a second error UI.
    fn fallback_frame(
        &self,
        scrollbar: Option<pain_terminal_core::selection::ScrollbarGeometry>,
        metrics: &CellMetrics,
        rows: u16,
    ) -> FrameState {
        let cache = self.cache.borrow();
        if cache.generation == 0 && cache.shaped_rows.is_empty() {
            let empty: Vec<Arc<CachedRow>> = (0..usize::from(rows))
                .map(|_| Arc::new(CachedRow::default()))
                .collect();
            FrameState {
                rows: Arc::new(empty),
                image_below_background: Arc::new(Vec::new()),
                image_below_text: Arc::new(Vec::new()),
                image_above_text: Arc::new(Vec::new()),
                cursor: None,
                preedit: None,
                line_height: px(metrics.line_height),
                background: cache.background,
                scrollbar,
            }
        } else {
            FrameState {
                rows: cache.shaped_rows.clone(),
                image_below_background: cache.image_below_background.clone(),
                image_below_text: cache.image_below_text.clone(),
                image_above_text: cache.image_above_text.clone(),
                cursor: cache.cursor.clone(),
                preedit: None,
                line_height: cache.line_height,
                background: cache.background,
                scrollbar,
            }
        }
    }

    /// Restart the epoch-based cursor timer. No timer runs for a
    /// non-blinking cursor or an inactive window.
    fn restart_blink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let blinking = self.blink.borrow().blinking;
        let epoch = self.blink.borrow_mut().restart();
        cx.notify();
        if !blinking || !window.is_window_active() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| loop {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            let keep = this
                .update_in(cx, |this, window, cx| {
                    if !window.is_window_active() {
                        this.stop_blink(cx);
                        return false;
                    }
                    let running = this.blink.borrow_mut().tick(epoch);
                    if running {
                        // Only cursor paint state changes; rows stay shared.
                        cx.notify();
                    }
                    running
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        })
        .detach();
    }

    fn stop_blink(&mut self, cx: &mut Context<Self>) {
        self.blink.borrow_mut().stop();
        cx.notify();
    }

    /// Schedule exactly one timer for the next Kitty animation deadline. A
    /// stopped animation invalidates any outstanding timer; a superseded
    /// deadline gets a new epoch, so the old callback cannot resurrect it.
    fn schedule_kitty_animation(
        &mut self,
        next: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(next_ms) = next else {
            self.kitty.borrow_mut().invalidate();
            return;
        };
        let epoch = {
            let mut kitty = self.kitty.borrow_mut();
            if kitty.scheduled && kitty.next_ms == Some(next_ms) {
                return;
            }
            kitty.epoch = kitty.epoch.wrapping_add(1);
            kitty.scheduled = true;
            kitty.pending = false;
            kitty.next_ms = Some(next_ms);
            kitty.epoch
        };
        let now = self.session.kitty_clock_ms();
        let delay_ms = next_ms.saturating_sub(now).max(1);
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay_ms))
                .await;
            let _ = this.update_in(cx, |this, _window, cx| {
                let mut kitty = this.kitty.borrow_mut();
                if kitty.epoch != epoch {
                    return;
                }
                kitty.scheduled = false;
                kitty.pending = true;
                cx.notify();
            });
        })
        .detach();
    }

    /// Composition callbacks only update local preedit; no PTY write here.
    fn set_preedit(
        &mut self,
        text: &str,
        selected: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        *self.preedit.borrow_mut() = PreeditState {
            text: text.to_owned(),
            selected,
        };
        cx.notify();
    }

    fn clear_preedit(&mut self, cx: &mut Context<Self>) {
        let was_active = !self.preedit.borrow().text.is_empty();
        *self.preedit.borrow_mut() = PreeditState::default();
        if was_active {
            cx.notify();
        }
    }

    fn marked_range(&self) -> Option<std::ops::Range<usize>> {
        let preedit = self.preedit.borrow();
        if preedit.text.is_empty() {
            None
        } else {
            Some(0..preedit.text.encode_utf16().count())
        }
    }

    /// Candidate window position: the terminal cursor cell from the canonical
    /// geometry, never the window's top-left corner.
    fn caret_bounds(&self) -> Option<Bounds<Pixels>> {
        let geometry = *self.geometry.borrow();
        if geometry.cols == 0 || geometry.rows == 0 {
            return None;
        }
        let (col, row) = self.cursor_cell.get();
        let (x, y, w, h) = geometry.span_bounds(col, row, 1);
        Some(Bounds::new(
            point(px(x), px(y)),
            size(px(w.max(1.0)), px(h.max(1.0))),
        ))
    }

    fn cancel_pointer_capture(&mut self, cx: &mut Context<Self>) {
        self.capture = PointerCapture::None;
        self.held_mouse.set(0);
        self.held_keys.borrow_mut().clear();
        self.last_drag_cell.set(None);
        *self.selection_anchor.borrow_mut() = None;
        self.session.cancel_selection_gesture();
        cx.notify();
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

    /// Apply a host theme change. Pain owns Silvestris; the view only receives
    /// the resolved monospace family and size, so other hosts can supply their
    /// own without forcing a Silvestris dependency here.
    pub fn set_font(&mut self, font: TerminalFont, cx: &mut Context<Self>) {
        self.font_family = font.family;
        self.default_font_size = font.size;
        self.font_size = font.size;
        cx.notify();
    }

    /// Install the host's shortcut ownership predicate. The early interceptor
    /// and this view then agree on which keys never reach the PTY.
    pub fn set_shortcut_filter(&mut self, filter: ShortcutFilter) {
        *self.shortcut_filter.borrow_mut() = filter;
    }

    fn cell_at(&self, position: Point<Pixels>) -> (u16, u16) {
        self.geometry
            .borrow()
            .cell_at((f32::from(position.x), f32::from(position.y)))
    }

    /// Control keys are encoded by Ghostty against the terminal's current
    /// modes. Plain printable text propagates to GPUI's native text handler.
    /// App shortcuts (including font keys) are claimed by the raw interceptor
    /// and dispatched as actions, so this listener must not apply them again.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.restart_blink(window, cx);
        let name = event.keystroke.key.as_str();
        let m = event.keystroke.modifiers;
        let mods = InputModifiers::new(m.shift, m.control, m.alt, m.platform);
        // GPUI dispatches these bindings before this listener. Let the action
        // own the key entirely; encoding it too would send a stray sequence
        // to the PTY after a clipboard copy or paste.
        if (self.shortcut_filter.borrow())(name, event.keystroke.key_char.as_deref(), mods) {
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
            if let Err(error) = self.session.encode_key(&key_event) {
                self.report_error_once(format!("key input rejected: {error}"));
            }
            self.held_keys
                .borrow_mut()
                .push((name.to_owned(), encoded.clone()));
        }
        cx.stop_propagation();
    }

    /// Kitty `report_events` releases. Only a key whose press was encoded is
    /// released; the release's own modifiers are preserved.
    fn on_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        let name = event.keystroke.key.as_str();
        let index = self
            .held_keys
            .borrow()
            .iter()
            .position(|(held_name, _)| held_name == name);
        let Some(index) = index else {
            return;
        };
        let (_, press) = self.held_keys.borrow_mut().remove(index);
        let m = event.keystroke.modifiers;
        let release_mods = InputModifiers::new(m.shift, m.control, m.alt, m.platform);
        let Some(encoded) =
            key_release_for_keystroke(name, event.keystroke.key_char.as_deref(), release_mods)
        else {
            return;
        };
        let Ok(mut key_event) = KeyEvent::new() else {
            return;
        };
        let _ = key_event.release(encoded.key, release_mods.ghostty_bits());
        if let Some(text) = &press.text {
            let _ = key_event.set_utf8(text);
        }
        let _ = key_event.set_unshifted_codepoint(press.unshifted_codepoint);
        if let Err(error) = self.session.encode_key(&key_event) {
            self.report_error_once(format!("key release rejected: {error}"));
        }
    }

    // Action listeners live on this focused element, matching Chaus. A global
    // `cx.on_action` handler would have to call `WindowHandle::update` while
    // the window is already leased by GPUI's key dispatch, which fails and
    // leaves the shortcut dead.
    fn increase_font(&mut self, _: &IncreaseFont, _window: &mut Window, cx: &mut Context<Self>) {
        self.adjust_font(FONT_SIZE_STEP, cx);
        cx.stop_propagation();
    }

    fn decrease_font(&mut self, _: &DecreaseFont, _window: &mut Window, cx: &mut Context<Self>) {
        self.adjust_font(-FONT_SIZE_STEP, cx);
        cx.stop_propagation();
    }

    fn reset_font(&mut self, _: &ResetFont, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_font_size(self.default_font_size, cx);
        cx.stop_propagation();
    }

    fn copy_selection(&mut self, _: &CopySelection, _window: &mut Window, cx: &mut Context<Self>) {
        match self.session.copy_selection() {
            Ok(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Ok(_) => {}
            Err(error) => self.report_error_once(format!("copy failed: {error}")),
        }
        cx.stop_propagation();
    }

    fn paste_clipboard(
        &mut self,
        _: &PasteClipboard,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            if let Err(error) = self.session.paste(&text) {
                self.report_error_once(format!("paste failed: {error}"));
            }
        }
        cx.stop_propagation();
    }

    fn select_all(&mut self, _: &SelectAllTerminal, _window: &mut Window, cx: &mut Context<Self>) {
        if let Err(error) = self.session.select_all() {
            self.report_error_once(format!("select all failed: {error}"));
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
                f64::from(delta.y) * f64::from(self.geometry.borrow().cell_height)
            }
        };
        if up_pixels == 0.0 {
            return;
        }
        let line_height = self.geometry.borrow().cell_height;
        let (rows, remainder) = accumulate_wheel(self.scroll_remainder, up_pixels, line_height);
        self.scroll_remainder = remainder;
        if rows == 0 {
            return;
        }
        if !event.modifiers.platform {
            // One terminal mouse wheel report per whole row. The session owns
            // the tracking decision under the terminal lock.
            let button = wheel_button(rows < 0);
            let (x, _y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
            let mut reported = false;
            for _ in 0..rows.unsigned_abs() {
                let route = self.route_pointer(MouseInput {
                    action: MouseAction::Press.ghostty(),
                    button: Some(button),
                    mods: mods.ghostty_bits(),
                    x: px,
                    y: py,
                    any_button_pressed: self.held_mouse.get() != 0,
                    surface_width: w,
                    surface_height: h,
                    cell_width: self.geometry.borrow().cell_width.max(1.0) as u32,
                    cell_height: self.geometry.borrow().cell_height.max(1.0) as u32,
                });
                reported |= matches!(route, MouseRoute::Reported { .. });
                let _ = x;
            }
            if reported {
                cx.stop_propagation();
                return;
            }
        }
        // `accumulate_wheel` already returns Ghostty's delta convention:
        // negative rows move up into history.
        let _ = self.session.scroll(rows);
        cx.stop_propagation();
    }

    fn mouse_event(
        &self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) -> (u16, u16, u32, u32, f32, f32, InputModifiers) {
        let geometry = *self.geometry.borrow();
        let (x, y) = self.cell_at(position);
        let (surface_width, surface_height) = geometry.content_size_px();
        let mods = InputModifiers::new(
            modifiers.shift,
            modifiers.control,
            modifiers.alt,
            modifiers.platform,
        );
        (
            x,
            y,
            surface_width,
            surface_height,
            f32::from(position.x) - geometry.origin.0,
            f32::from(position.y) - geometry.origin.1,
            mods,
        )
    }

    /// One pointer event to the session: it decides routing and queues bytes
    /// under a single terminal lock. Failures are logged, never silently
    /// converted into local selection.
    fn route_pointer(&self, input: MouseInput) -> MouseRoute {
        match self.session.route_mouse(input) {
            Ok(route) => route,
            Err(error) => {
                self.report_error_once(format!("mouse routing failed: {error}"));
                MouseRoute::NotTracking
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pointer_input(
        &self,
        action: MouseAction,
        button: Option<PtyButton>,
        mods: InputModifiers,
        px: f32,
        py: f32,
        w: u32,
        h: u32,
        held: bool,
    ) -> MouseInput {
        let g = *self.geometry.borrow();
        MouseInput {
            action: action.ghostty(),
            button: button.and_then(PtyButton::ghostty),
            mods: mods.ghostty_bits(),
            x: px,
            y: py,
            any_button_pressed: held,
            surface_width: w,
            surface_height: h,
            cell_width: g.cell_width.max(1.0) as u32,
            cell_height: g.cell_height.max(1.0) as u32,
        }
    }

    fn drag_scrollbar_to(&self, y: f32) {
        let (bar, state) = (*self.scrollbar.borrow(), *self.scroll_state.borrow());
        if let (Some(bar), Some(state)) = (bar, state) {
            let row = scrollbar_row_at(&bar, y, state.total, state.len);
            // One lock/state transition. The returned scrollbar state replaces
            // the cached one; no second `session.scrollbar()` query is needed.
            if let Ok(outcome) = self.session.scroll_to_row(row) {
                *self.scroll_state.borrow_mut() = Some(outcome.scrollbar);
            }
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.restart_blink(window, cx);
        let (x, y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        if event.button == MouseButton::Left {
            if let Some(bar) = *self.scrollbar.borrow() {
                let local_x = f32::from(event.position.x) - bar.track_x;
                if (0.0..=bar.track_width).contains(&local_x) {
                    // Explicit UI pointer capture; no terminal button bit is
                    // set, so Ghostty never sees scrollbar drag reporting.
                    self.capture = PointerCapture::Scrollbar;
                    self.drag_scrollbar_to(f32::from(event.position.y));
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
        if !mouse_escape(mods) {
            let route = self.route_pointer(self.pointer_input(
                MouseAction::Press,
                Some(button),
                mods,
                px,
                py,
                w,
                h,
                held,
            ));
            if matches!(route, MouseRoute::Reported { .. }) {
                cx.stop_propagation();
                return;
            }
        }
        if button == PtyButton::Left {
            // Only the left button starts a local selection. Right/middle with
            // reporting off belong to the platform (context menu), not Ghostty.
            *self.selection_anchor.borrow_mut() = Some((x, y));
            self.last_drag_cell.set(Some((x, y)));
            if let Err(error) = self
                .session
                .begin_selection_gesture(x, y, event.click_count)
            {
                self.report_error_once(format!("gesture press failed: {error}"));
            }
            cx.stop_propagation();
        }
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.capture == PointerCapture::Scrollbar {
            self.drag_scrollbar_to(f32::from(event.position.y));
            return;
        }
        let (x, y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        let held = self.held_mouse.get() != 0;
        if !mouse_escape(mods) {
            let route = self.route_pointer(self.pointer_input(
                MouseAction::Motion,
                event.pressed_button.and_then(pty_button),
                mods,
                px,
                py,
                w,
                h,
                held,
            ));
            if matches!(route, MouseRoute::Reported { .. }) {
                return;
            }
        }
        if self.selection_anchor.borrow().is_some() {
            // Do not feed Ghostty another gesture event until the pointer
            // actually enters a different terminal cell.
            if self.last_drag_cell.get() != Some((x, y)) {
                self.last_drag_cell.set(Some((x, y)));
                let g = *self.geometry.borrow();
                let _ =
                    self.session
                        .drag_selection_gesture(x, y, g.cols, g.cell_width, g.cell_height);
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.capture == PointerCapture::Scrollbar {
            // Clear capture BEFORE returning: a release over the titlebar or
            // outside the window must not leave a permanent drag.
            self.capture = PointerCapture::None;
            return;
        }
        let (_x, _y, w, h, px, py, mods) = self.mouse_event(event.position, event.modifiers);
        let Some(button) = pty_button(event.button) else {
            return;
        };
        self.held_mouse.set(self.held_mouse.get() & !button.bit());
        let held = self.held_mouse.get() != 0;
        if !mouse_escape(mods) {
            let route = self.route_pointer(self.pointer_input(
                MouseAction::Release,
                Some(button),
                mods,
                px,
                py,
                w,
                h,
                held,
            ));
            if matches!(route, MouseRoute::Reported { .. }) {
                return;
            }
        }
        if button == PtyButton::Left {
            self.last_drag_cell.set(None);
            *self.selection_anchor.borrow_mut() = None;
            self.session.end_selection_gesture();
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
            .id(("terminal-view", self.view_id))
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context(KeyContext::new_with_defaults(cx.platform_info().os))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_action(cx.listener(Self::increase_font))
            .on_action(cx.listener(Self::decrease_font))
            .on_action(cx.listener(Self::reset_font))
            .on_action(cx.listener(Self::copy_selection))
            .on_action(cx.listener(Self::paste_clipboard))
            .on_action(cx.listener(Self::select_all))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
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

/// One helper for all three Ghostty image stages. `paint_image` derives the
/// atlas source subregion from the difference between `bounds` and
/// `image_bounds`; the source buffer is never cropped here.
fn paint_image_list(window: &mut Window, images: &[ImageDraw]) {
    for draw in images {
        let _ = window.paint_image(
            draw.bounds,
            draw.image_bounds,
            Corners::default(),
            Arc::clone(&draw.image),
            0,
            false,
        );
    }
}

struct TerminalElement {
    view: gpui::Entity<TerminalView>,
}

/// Everything paint needs, precomputed in prepaint. Rows are shared handles,
/// so a cursor blink frame can reuse every shaped row without copying them.
struct FrameState {
    rows: Arc<Vec<Arc<CachedRow>>>,
    /// Ghostty layer order: below backgrounds, below text, above text.
    image_below_background: Arc<Vec<ImageDraw>>,
    image_below_text: Arc<Vec<ImageDraw>>,
    image_above_text: Arc<Vec<ImageDraw>>,
    cursor: Option<PaintQuad>,
    /// Locally composed IME text, drawn at the caret but never in the PTY.
    preedit: Option<(Point<Pixels>, ShapedLine)>,
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

        let font_family = view.font_family.clone();
        // The host supplies the family and initial size; Ctrl+=/- changes only
        // the size override held by this view.
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
        // Use the font's own line box. GPUI centers each shaped row inside the
        // height passed to `ShapedLine::paint`, so extra leading would show up
        // as a gap between rows and break TUI box-drawing borders.
        let line_height = line_height_for(
            f32::from(probe.ascent),
            f32::from(probe.descent),
            f32::from(font_size),
        );
        let metrics = CellMetrics::new(cell_width, line_height);
        // The canonical geometry is written BEFORE any fast-path early return,
        // so input handlers always hit-test against the current frame's child
        // bounds even when no rows were rebuilt.
        let geometry = TerminalGeometry::new(
            (f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
            (
                f32::from(bounds.size.width).max(0.0),
                f32::from(bounds.size.height).max(0.0),
            ),
            &metrics,
            window.scale_factor(),
        );
        *view.geometry.borrow_mut() = geometry;
        let (cols, rows) = geometry.grid();

        // The scrollbar thumb is a view over Ghostty's row counts, never a GPUI
        // ScrollHandle or a second history model.
        let scroll_state = session.scrollbar().ok();
        let bar = scroll_state.and_then(|state| {
            scrollbar_geometry(
                state.total,
                state.offset,
                state.len,
                geometry.origin.0,
                geometry.origin.1,
                geometry.content_width,
                geometry.content_height,
            )
        });
        *view.scrollbar.borrow_mut() = bar;
        *view.scroll_state.borrow_mut() = scroll_state;

        let wake = session.take_wakeup();
        let generation = session.generation();
        // A due Kitty animation must never take the cached-frame path just
        // because PTY output stopped. The flag is consumed here so a later
        // fast path is allowed again once this frame rendered.
        let animation_due = view.kitty.borrow_mut().take_pending();

        {
            let cache = view.cache.borrow();
            // A pixel-only window resize leaves the cached rows valid: their
            // quads and text origins depend on the content origin and cell
            // metrics, not on the window's width/height. The cached grid is
            // compared explicitly; `shaped_rows.len()` is never used to infer
            // cols or rows.
            if wake.is_none()
                && !animation_due
                && cache.generation == generation
                && cache.geometry == geometry
                && cache.cols == cols
                && cache.rows == rows
                && cache.preedit_text == view.preedit.borrow().text
            {
                counters.prepaint_micros(
                    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
                );
                return FrameState {
                    rows: cache.shaped_rows.clone(),
                    image_below_background: cache.image_below_background.clone(),
                    image_below_text: cache.image_below_text.clone(),
                    image_above_text: cache.image_above_text.clone(),
                    cursor: cache.cursor.clone(),
                    preedit: None,
                    line_height: cache.line_height,
                    background: cache.background,
                    scrollbar: bar,
                };
            }
        }

        // One animation tick per rendered frame. This both advances a due
        // frame and discovers a newly started animation. It mutates Kitty
        // storage, so it must happen before render-state extraction and pixel
        // lookup below.
        let kitty_next = session
            .tick_kitty_animation(session.kitty_clock_ms())
            .ok()
            .flatten();

        // Apply layout geometry synchronously: this frame paints the new
        // state, so the session must not enqueue a redundant renderer wake.
        // Ghostty's cell geometry is terminal pixels; GPUI drawing below stays
        // in logical pixels.
        let (cell_width_device, cell_height_device) = geometry.cell_device_pixels();
        let outcome = match session.apply_layout_geometry(
            cols,
            rows,
            cell_width_device,
            cell_height_device,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                // Preserve the previous frame and do not mark it complete: the
                // failed geometry leaves Ghostty dirty for a later recovery.
                view.report_error_once(error);
                counters.prepaint_micros(
                    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
                );
                return view.fallback_frame(bar, &metrics, rows);
            }
        };
        // The host owns the PTY relationship. Report only real grid changes,
        // with the dimensions Ghostty adopted; never call transport code here.
        if outcome.grid_changed {
            if let Some(sink) = view.grid_change.borrow().as_ref() {
                sink(
                    outcome.geometry.cols,
                    outcome.geometry.rows,
                    outcome.geometry.cell_width_px,
                    outcome.geometry.cell_height_px,
                );
            }
        }
        let snapshot = match session.render_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // Leave Ghostty dirty: no `mark_frame_complete` for a frame we
                // did not capture. The next successful frame rebuilds it.
                view.report_error_once(error);
                counters.prepaint_micros(
                    u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX),
                );
                return view.fallback_frame(bar, &metrics, rows);
            }
        };
        // Upload only missing generations and drop stale textures. This never
        // touches Ghostty image pointers past the terminal lock.
        let image_layers = view.images.borrow_mut().prepare(
            &session,
            &snapshot.image_placements,
            &geometry,
            &counters,
            window,
        );
        let rebuild: Vec<usize> = {
            let mut cache = view.cache.borrow_mut();
            let listed =
                cache
                    .row_cache
                    .rows_to_rebuild(generation, snapshot.dirty, &snapshot.dirty_rows);
            let invalid = cache.geometry != geometry || cache.cols != cols || cache.rows != rows;
            if invalid {
                cache.row_cache = RowCache::new(usize::from(rows));
                let _ = cache
                    .row_cache
                    .rows_to_rebuild(generation, snapshot.dirty, &[]);
                cache.shaped_rows = Arc::new(
                    (0..usize::from(rows))
                        .map(|_| Arc::new(CachedRow::default()))
                        .collect(),
                );
                (0..usize::from(rows)).collect()
            } else {
                listed
            }
        };
        let palette = &snapshot.colors.palette;
        let default_bg = rgb(snapshot.colors.background);
        let default_fg = rgb(snapshot.colors.foreground);
        let origin = geometry.origin;

        // Mutate the canonical cache in place. Only Ghostty's dirty row indices
        // are reshaped; every other shared row handle survives untouched.
        let mut dirty = vec![false; usize::from(rows)];
        for &y in &rebuild {
            if let Some(slot) = dirty.get_mut(y) {
                *slot = true;
            }
        }
        {
            let mut cache = view.cache.borrow_mut();
            if Arc::get_mut(&mut cache.shaped_rows).is_none() {
                // One cheap Vec-of-Arcs copy when a previous frame still holds
                // the handle; never a deep copy of shaped rows.
                counters.row_cache_copies(1);
            }
            let shaped = Arc::make_mut(&mut cache.shaped_rows);
            shaped.resize_with(usize::from(rows), || Arc::new(CachedRow::default()));
            for y in 0..usize::from(rows) {
                if !dirty[y] {
                    counters.row_cache_hits(1);
                    continue;
                }
                let cells = snapshot.line(y);
                let mut quads = Vec::new();
                let mut lines = Vec::new();

                for run_bg in
                    background_runs(cells, default_spec(snapshot.colors.background), palette)
                {
                    quads.push(bg_quad(&metrics, origin, y, run_bg));
                    counters.background_quads(1);
                }
                if let Some((start_x, cells_len)) =
                    snapshot.selection(y).and_then(|s| selection_span(Some(s)))
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
                shaped[y] = Arc::new(CachedRow { quads, lines });
                counters.rows_rebuilt(1);
            }
        }

        view.cursor_cell.set((snapshot.cursor.x, snapshot.cursor.y));
        let cursor_blinking = snapshot.cursor.blinking;
        let blink_changed = view.blink.borrow().blinking != cursor_blinking;
        if blink_changed {
            view.blink.borrow_mut().blinking = cursor_blinking;
        }
        let cursor_visible = !cursor_blinking || view.blink.borrow().on;
        let cursor = if snapshot.cursor.visible && snapshot.cursor.has_value && cursor_visible {
            cursor_quad(&metrics, origin, &snapshot.cursor, default_fg)
        } else {
            None
        };

        // Locally composed text is drawn at the caret. It is never part of a
        // shaped terminal row and never reaches the PTY.
        let preedit = {
            let state = view.preedit.borrow();
            if state.text.is_empty() {
                None
            } else {
                let text: SharedString = state.text.clone().into();
                let run = TextRun {
                    len: text.len(),
                    font: mono_font.clone(),
                    color: default_fg,
                    ..Default::default()
                };
                let shaped = window.text_system().shape_line(
                    text,
                    font_size,
                    &[run],
                    Some(px(metrics.cell_width)),
                );
                counters.shape_line(state.text.len());
                let (x, y) =
                    geometry.cell_origin(view.cursor_cell.get().0, view.cursor_cell.get().1);
                Some((point(px(x), px(y)), shaped))
            }
        };
        let preedit_text = view.preedit.borrow().text.clone();
        let image_below_background = Arc::new(image_layers.below_background);
        let image_below_text = Arc::new(image_layers.below_text);
        let image_above_text = Arc::new(image_layers.above_text);

        // Only a fully captured frame may clear Ghostty's dirty flags. If the
        // clean call fails, keep the prior cache and leave the state dirty so
        // the next frame can rebuild it.
        if let Err(error) = session.mark_frame_complete() {
            view.report_error_once(error);
            counters
                .prepaint_micros(u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX));
            return view.fallback_frame(bar, &metrics, rows);
        }
        let frame_rows = view.cache.borrow().shaped_rows.clone();
        {
            let mut cache = view.cache.borrow_mut();
            cache.generation = generation;
            cache.geometry = geometry;
            cache.cols = cols;
            cache.rows = rows;
            cache.preedit_text = preedit_text;
            cache.cursor = cursor.clone();
            cache.line_height = px(line_height);
            cache.background = default_bg;
            cache.image_below_background = image_below_background.clone();
            cache.image_below_text = image_below_text.clone();
            cache.image_above_text = image_above_text.clone();
        }
        counters.prepaint_micros(u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX));
        if blink_changed {
            let entity = self.view.clone();
            entity.update(cx, |this, cx| {
                if cursor_blinking && this.focus.is_focused(window) {
                    this.restart_blink(window, cx);
                } else {
                    this.stop_blink(cx);
                }
            });
        }
        {
            let entity = self.view.clone();
            entity.update(cx, |this, cx| {
                this.schedule_kitty_animation(kitty_next, window, cx);
            });
        }
        FrameState {
            rows: frame_rows,
            image_below_background,
            image_below_text,
            image_above_text,
            cursor,
            preedit,
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
        paint_image_list(window, &prepaint.image_below_background);
        for row in prepaint.rows.iter() {
            for quad in &row.quads {
                window.paint_quad(quad.clone());
            }
        }
        paint_image_list(window, &prepaint.image_below_text);
        for row in prepaint.rows.iter() {
            for (origin, line) in &row.lines {
                if let Err(error) = line.paint(
                    *origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                ) {
                    // A single unshapable row must not crash the pane or the
                    // window; skip it and report once to the host.
                    self.view
                        .read(cx)
                        .report_error_once(format!("terminal line paint failed: {error}"));
                }
            }
        }
        paint_image_list(window, &prepaint.image_above_text);
        if let Some(cursor) = &prepaint.cursor {
            window.paint_quad(cursor.clone());
        }
        if let Some((origin, line)) = &prepaint.preedit {
            if let Err(error) = line.paint(
                *origin,
                prepaint.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            ) {
                self.view
                    .read(cx)
                    .report_error_once(format!("IME preedit paint failed: {error}"));
            }
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
        // the element. A window-level listener continues a drag that leaves
        // the surface when EITHER an explicit scrollbar capture or a terminal
        // mouse button is held. It never calls stop_propagation, so Silvestris
        // titlebar and traffic-light controls still receive their own events.
        let mouse_view = self.view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let view = mouse_view.read(cx);
            if view.capture == PointerCapture::None && view.held_mouse.get() == 0 {
                return;
            }
            let g = *view.geometry.borrow();
            let terminal_bounds = Bounds::new(
                point(px(g.origin.0), px(g.origin.1)),
                size(px(g.content_width), px(g.content_height)),
            );
            // The element-level listener handles moves inside the surface.
            if view.capture != PointerCapture::Scrollbar
                && terminal_bounds.contains(&event.position)
            {
                return;
            }
            mouse_view.update(cx, |view, cx| view.on_mouse_move(event, window, cx));
        });
        let mouse_view = self.view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let view = mouse_view.read(cx);
            if view.capture == PointerCapture::None && view.held_mouse.get() == 0 {
                return;
            }
            mouse_view.update(cx, |view, cx| view.on_mouse_up(event, window, cx));
        });
        let view = self.view.read(cx);
        window.handle_input(
            &view.focus,
            TerminalInputHandler {
                session: Arc::clone(&view.session),
                view: self.view.clone(),
            },
            cx,
        );
        view.counters
            .paint_micros(u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
}

/// Native text/IME bridge. Only committed text goes to the PTY; marked text is
/// held by the view for candidate placement. Paste goes through Ghostty's
/// bracketed-paste policy.
struct TerminalInputHandler {
    session: Arc<TerminalSession>,
    view: gpui::Entity<TerminalView>,
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
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.view.read(cx).marked_range()
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

    /// Commit path. Windows calls this with the completed reading (and with an
    /// empty string when a composition is cancelled), so only non-empty final
    /// text is forwarded to the child.
    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        if pain_terminal_core::input::should_commit_text(false, text) {
            if let Err(error) = self.session.write_input(text) {
                self.view.update(cx, |view, _| {
                    view.report_error_once(format!("committed text rejected: {error}"));
                });
            }
        }
        self.view.update(cx, |view, cx| view.clear_preedit(cx));
    }

    /// Composition path: update local preedit only. Never write to the PTY and
    /// never treat an intermediate reading as a paste.
    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        self.view.update(cx, |view, cx| {
            view.set_preedit(new_text, new_selected_range.map(|r| (r.start, r.end)), cx);
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        self.view.update(cx, |view, cx| view.clear_preedit(cx));
    }

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, cx: &mut App) {
        if let Some(text) = item.text() {
            if let Err(error) = self.session.paste(&text) {
                self.view.update(cx, |view, _| {
                    view.report_error_once(format!("paste failed: {error}"));
                });
            }
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.view.update(cx, |view, _| view.caret_bounds())
    }

    /// There is no mapping from arbitrary screen points into Ghostty's
    /// terminal document, so do not claim one.
    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }
}

fn default_spec(rgb: (u8, u8, u8)) -> pain_terminal_core::ghostty::ColorSpec {
    pain_terminal_core::ghostty::ColorSpec::Rgb(rgb.0, rgb.1, rgb.2)
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

// Shaping parameters are recorded explicitly rather than wrapped in a struct
// used by one caller; the count mirrors GPUI's text-run inputs.
#[allow(clippy::too_many_arguments)]
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
