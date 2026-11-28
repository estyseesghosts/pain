//! GPUI pointer subsystem for `TerminalView`.
//!
//! This child module owns the GPUI-event-to-terminal adaptation, held-button
//! policy, terminal-vs-local selection routing, wheel reporting and scrollbar
//! capture. It changes no terminal semantics: core still decides whether an
//! event is reported and encodes it, and the render element still owns layout,
//! prepaint and paint. All pointer state stays on the one `TerminalView`.

use gpui::{
    Context, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta,
    ScrollEvent, Window,
};
use pain_terminal_core::{
    accumulate_wheel, mouse_escape, scrollbar_row_at, wheel_button, Modifiers as InputModifiers,
    MouseAction, MouseButton as PtyButton, MouseInput, MouseRoute, PointerSample,
};

use super::{TerminalView, TerminalViewOperation};

/// Explicit pointer capture. Scrollbar dragging is a UI-domain gesture and
/// must never be represented by a fake terminal mouse-button bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PointerCapture {
    None,
    Scrollbar,
}

impl TerminalView {
    pub(super) fn cancel_pointer_capture(&mut self, cx: &mut Context<Self>) {
        self.capture = PointerCapture::None;
        self.held_mouse.set(0);
        self.held_keys.borrow_mut().clear();
        self.last_drag_cell.set(None);
        *self.selection_anchor.borrow_mut() = None;
        self.session.cancel_selection_gesture();
        cx.notify();
    }

    /// Wheel/trackpad scrolling. When a TUI reports mouse tracking and no
    /// escape modifier is held, wheel events are encoded for the child.
    /// Otherwise they move Ghostty's viewport through its own scroll API.
    pub(super) fn on_scroll(
        &mut self,
        event: &ScrollEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            let (pointer, mods) = self.pointer_event(event.position, event.modifiers);
            let mut reported = false;
            for _ in 0..rows.unsigned_abs() {
                let route = self.route_pointer(self.pointer_input(
                    MouseAction::Press,
                    Some(button),
                    mods,
                    pointer,
                    self.held_mouse.get() != 0,
                ));
                reported |= matches!(route, MouseRoute::Reported { .. });
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

    /// One GPUI event → one canonical pointer sample. No caller derives a
    /// cell, a surface position or a cell size a second time: they all read
    /// this same normalized value.
    fn pointer_event(
        &self,
        position: Point<Pixels>,
        modifiers: gpui::Modifiers,
    ) -> (PointerSample, InputModifiers) {
        let geometry = *self.geometry.borrow();
        let pointer = geometry.pointer_sample((f32::from(position.x), f32::from(position.y)));
        let mods = InputModifiers::new(
            modifiers.shift,
            modifiers.control,
            modifiers.alt,
            modifiers.platform,
        );
        (pointer, mods)
    }

    /// One pointer event to the session: it decides routing and queues bytes
    /// under a single terminal lock. Failures are reported as discrete pointer
    /// actions, never silently converted into local selection.
    fn route_pointer(&self, input: MouseInput) -> MouseRoute {
        match self.session.route_mouse(input) {
            Ok(route) => route,
            Err(error) => {
                self.report_action_error(
                    TerminalViewOperation::Pointer,
                    format!("mouse routing failed: {error}"),
                );
                MouseRoute::NotTracking
            }
        }
    }

    /// Wrap one pointer sample in a domain mouse event. Ghostty's numeric
    /// action/button codes are added by core, not by the view.
    fn pointer_input(
        &self,
        action: MouseAction,
        button: Option<PtyButton>,
        mods: InputModifiers,
        pointer: PointerSample,
        held: bool,
    ) -> MouseInput {
        MouseInput {
            action,
            button,
            mods,
            pointer,
            any_button_pressed: held,
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

    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.restart_blink(window, cx);
        let (pointer, mods) = self.pointer_event(event.position, event.modifiers);
        let (x, y) = pointer.cell;
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
                pointer,
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
                self.report_action_error(
                    TerminalViewOperation::Selection,
                    format!("gesture press failed: {error}"),
                );
            }
            cx.stop_propagation();
        }
    }

    pub(super) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.capture == PointerCapture::Scrollbar {
            self.drag_scrollbar_to(f32::from(event.position.y));
            return;
        }
        let (pointer, mods) = self.pointer_event(event.position, event.modifiers);
        let (x, y) = pointer.cell;
        let held = self.held_mouse.get() != 0;
        if !mouse_escape(mods) {
            let route = self.route_pointer(self.pointer_input(
                MouseAction::Motion,
                event.pressed_button.and_then(pty_button),
                mods,
                pointer,
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
                let _ = self.session.drag_selection_gesture(x, y, pointer);
            }
        }
    }

    pub(super) fn on_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.capture == PointerCapture::Scrollbar {
            // Clear capture BEFORE returning: a release over the titlebar or
            // outside the window must not leave a permanent drag.
            self.capture = PointerCapture::None;
            return;
        }
        let (pointer, mods) = self.pointer_event(event.position, event.modifiers);
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
                pointer,
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

/// GPUI button → terminal-domain button. `Navigate` has no terminal protocol
/// representation and is left to the platform.
fn pty_button(button: MouseButton) -> Option<PtyButton> {
    match button {
        MouseButton::Left => Some(PtyButton::Left),
        MouseButton::Right => Some(PtyButton::Right),
        MouseButton::Middle => Some(PtyButton::Middle),
        MouseButton::Navigate(_) => None,
    }
}
