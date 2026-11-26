//! Safe, owning wrapper around the narrow `ghostty-vt-sys` surface.
//!
//! Every Ghostty handle is owned exactly once by a Rust value. `Drop` takes
//! the raw handle before freeing it, so a double free cannot happen through
//! this API. Raw handles are `Send` only so M2's single `TerminalSession` lock
//! can move them to the reader thread; they are never `Sync` or shared
//! directly. All access is serialized through that one lock.

use std::ffi::{c_void, CStr};
use std::fmt;
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use ghostty_vt_sys as sys;
pub use ghostty_vt_sys;

use crate::terminal::writer::WriterQueue;

/// Per-row selection range in viewport columns, queried once per row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowSelection {
    pub start_x: u16,
    pub end_x: u16,
}

impl RowSelection {
    #[must_use]
    pub fn contains(&self, x: u16) -> bool {
        x >= self.start_x && x <= self.end_x
    }
}

/// Resolved palette and default colors captured with a render snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colors {
    pub background: (u8, u8, u8),
    pub foreground: (u8, u8, u8),
    pub cursor: (u8, u8, u8),
    pub cursor_has_value: bool,
    pub palette: [(u8, u8, u8); 256],
}

/// A `GhosttyResult` that is not `GHOSTTY_SUCCESS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyError(pub sys::GhosttyResult);

impl fmt::Display for GhosttyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "libghostty-vt call failed with code {}", self.0)
    }
}

impl std::error::Error for GhosttyError {}

fn check(result: sys::GhosttyResult) -> Result<(), GhosttyError> {
    if result == sys::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(GhosttyError(result))
    }
}

/// The runtime version of the linked static library.
pub fn version() -> &'static str {
    // SAFETY: `ghostty_version` returns a pointer to a NUL-terminated static
    // string with the lifetime of the process; we never mutate or free it.
    let raw = unsafe { sys::ghostty_version() };
    if raw.is_null() {
        return "unknown";
    }
    // SAFETY: the pointer is non-null and points at a valid C string (above).
    unsafe { CStr::from_ptr(raw) }
        .to_str()
        .unwrap_or("invalid-utf8")
}

/// Plain Rust view of a cell's foreground or background color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpec {
    Palette(u8),
    Rgb(u8, u8, u8),
}

/// Plain Rust view of `GhosttyStyle` flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub flags: u16,
    pub foreground: Option<ColorSpec>,
    pub background: Option<ColorSpec>,
    pub underline_color: Option<ColorSpec>,
}

impl Style {
    fn from_raw(raw: sys::GhosttyStyle) -> Self {
        fn color(raw: sys::GhosttyStyleColor) -> Option<ColorSpec> {
            match raw.tag {
                sys::GHOSTTY_STYLE_COLOR_PALETTE => {
                    // SAFETY: tag `PALETTE` is the discriminant for the
                    // `palette` union member, so this is the active field.
                    Some(ColorSpec::Palette(unsafe { raw.value.palette }))
                }
                sys::GHOSTTY_STYLE_COLOR_RGB => {
                    // SAFETY: tag `RGB` is the discriminant for the `rgb`
                    // union member, so this is the active field.
                    let rgb = unsafe { raw.value.rgb };
                    Some(ColorSpec::Rgb(rgb.r, rgb.g, rgb.b))
                }
                _ => None,
            }
        }

        Self {
            flags: raw.flags,
            foreground: color(raw.fg_color),
            background: color(raw.bg_color),
            underline_color: color(raw.underline_color),
        }
    }

    #[must_use]
    pub fn is_bold(&self) -> bool {
        self.flags & sys::GHOSTTY_STYLE_FLAG_BOLD != 0
    }

    #[must_use]
    pub fn underline(&self) -> u16 {
        (self.flags & sys::GHOSTTY_STYLE_UNDERLINE_MASK) >> sys::GHOSTTY_STYLE_UNDERLINE_SHIFT
    }
}

/// One visible terminal cell, copied out of render state while it is valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub style: Style,
    pub foreground: Option<(u8, u8, u8)>,
    pub background: Option<(u8, u8, u8)>,
    pub selected: bool,
    pub has_styling: bool,
}

/// Cursor state captured in a render-state update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub has_value: bool,
    pub x: u16,
    pub y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: i32,
}

/// A complete, owned copy of the visible terminal state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub dirty: sys::GhosttyResult,
    pub cursor: Cursor,
    pub colors: Colors,
    pub lines: Vec<Vec<Cell>>,
    pub selections: Vec<Option<RowSelection>>,
    pub dirty_rows: Vec<u16>,
}

/// Owns one `GhosttyTerminal` handle.
pub struct GhosttyTerminal {
    raw: Option<sys::GhosttyTerminal>,
    cols: u16,
    rows: u16,
    /// Shared with the effect callbacks so `CSI ? 996 n` answers match the
    /// active light/dark palette.
    color_scheme: Arc<AtomicU8>,
}

impl GhosttyTerminal {
    /// Create an 80x24-style terminal. `cols`/`rows` are the initial grid.
    pub fn new(cols: u16, rows: u16) -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyTerminal = ptr::null_mut();
        // SAFETY: `raw` is a valid out-parameter and the allocator pointer is
        // NULL, which selects Ghostty's default allocator. On success the
        // returned handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_terminal_new(ptr::null(), &mut raw, cols, rows) })?;
        Ok(Self {
            raw: Some(raw),
            cols,
            rows,
            color_scheme: Arc::new(AtomicU8::new(0)),
        })
    }

    #[must_use]
    pub fn cols(&self) -> u16 {
        self.cols
    }

    #[must_use]
    pub fn rows(&self) -> u16 {
        self.rows
    }

    pub(crate) fn as_raw(&self) -> sys::GhosttyTerminal {
        self.raw.expect("owned terminal handle is still present")
    }

    /// Feed VT bytes into the terminal. No PTY or drawing is involved.
    pub fn write(&mut self, bytes: &[u8]) {
        // SAFETY: `as_raw` is non-null and uniquely owned; `bytes` stays alive
        // for the duration of this call and the C function only reads it.
        unsafe { sys::ghostty_terminal_vt_write(self.as_raw(), bytes.as_ptr(), bytes.len()) };
    }

    /// Resize the grid. Cell pixel dimensions are accepted because future
    /// protocols need them, but M0 derives them from nothing yet.
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), GhosttyError> {
        // SAFETY: the handle is uniquely owned and valid for this call.
        check(unsafe {
            sys::ghostty_terminal_resize(self.as_raw(), cols, rows, cell_width_px, cell_height_px)
        })?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// Apply a terminal palette. Ghostty generates the 240 non-base colours
    /// from the base-16 entries, then stores the result as its default palette;
    /// OSC overrides from child programs still take precedence at runtime.
    pub fn set_colors(
        &mut self,
        colors: Option<&crate::terminal::palette::Palette>,
    ) -> Result<(), GhosttyError> {
        let Some(colors) = colors else {
            // Setting explicit defaults matters: clearing the options leaves
            // the render state holding the previous palette's background.
            let mut palette = [sys::GhosttyColorRgb::default(); 256];
            // SAFETY: `palette` has the 256 entries the function writes.
            unsafe { sys::ghostty_color_palette_default(palette.as_mut_ptr()) };
            let black = sys::GhosttyColorRgb::default();
            let white = sys::GhosttyColorRgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            };
            // SAFETY: the option value types match the header and Ghostty
            // copies them before returning.
            unsafe {
                for (option, value) in [
                    (
                        sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                        &white as *const sys::GhosttyColorRgb as *const c_void,
                    ),
                    (
                        sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                        &black as *const sys::GhosttyColorRgb as *const c_void,
                    ),
                    (
                        sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                        &white as *const sys::GhosttyColorRgb as *const c_void,
                    ),
                    (
                        sys::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                        palette.as_ptr() as *const c_void,
                    ),
                ] {
                    check(sys::ghostty_terminal_set(self.as_raw(), option, value))?;
                }
            }
            self.color_scheme
                .store(sys::GHOSTTY_COLOR_SCHEME_DARK as u8, Ordering::Release);
            return Ok(());
        };
        let rgb = |c: crate::terminal::palette::Rgb| sys::GhosttyColorRgb {
            r: c.r,
            g: c.g,
            b: c.b,
        };
        let mut base = [sys::GhosttyColorRgb::default(); 256];
        for (slot, color) in base.iter_mut().zip(colors.base.iter()) {
            *slot = rgb(*color);
        }
        let mut palette = [sys::GhosttyColorRgb::default(); 256];
        let bg = rgb(colors.background);
        let fg = rgb(colors.foreground);
        let cursor = rgb(colors.cursor);
        // SAFETY: `base` and `palette` are 256-element arrays, `skip` is NULL as
        // documented, and every pointer is alive for the duration of the call.
        unsafe {
            sys::ghostty_color_palette_generate(
                base.as_ptr(),
                ptr::null(),
                &bg,
                &fg,
                false,
                palette.as_mut_ptr(),
            );
        }
        // SAFETY: each option's value type is the one documented in
        // `terminal.h` (`GhosttyColorRgb*` and `GhosttyColorRgb[256]*`). Ghostty
        // copies the values, so these locals may go out of scope immediately.
        let set = |option: i32, value: *const c_void| unsafe {
            check(sys::ghostty_terminal_set(self.as_raw(), option, value))
        };
        set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            &fg as *const sys::GhosttyColorRgb as *const c_void,
        )?;
        set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            &bg as *const sys::GhosttyColorRgb as *const c_void,
        )?;
        set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
            &cursor as *const sys::GhosttyColorRgb as *const c_void,
        )?;
        set(
            sys::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
            palette.as_ptr() as *const c_void,
        )?;
        self.color_scheme.store(
            if colors.dark {
                sys::GHOSTTY_COLOR_SCHEME_DARK
            } else {
                sys::GHOSTTY_COLOR_SCHEME_LIGHT
            } as u8,
            Ordering::Release,
        );
        Ok(())
    }

    /// Install a callback/option. Used for MVP effects such as `WRITE_PTY`.
    ///
    /// # Safety
    /// `value` must have the type the Ghostty option expects and must remain
    /// valid for as long as the terminal can call it.
    pub unsafe fn set_option(
        &mut self,
        option: i32,
        value: *const c_void,
    ) -> Result<(), GhosttyError> {
        // SAFETY: the caller upholds the option's value contract; the handle
        // is uniquely owned and valid for this call.
        check(unsafe { sys::ghostty_terminal_set(self.as_raw(), option, value) })
    }

    pub fn get_u16(&self, data: i32) -> Result<u16, GhosttyError> {
        let mut out = 0u16;
        // SAFETY: the handle is valid and `out` points at a `uint16_t`, which
        // is the type documented for the queried data keys.
        check(unsafe {
            sys::ghostty_terminal_get(self.as_raw(), data, &mut out as *mut u16 as *mut c_void)
        })?;
        Ok(out)
    }

    /// Query a terminal mode (e.g. bracketed paste) from Ghostty state.
    pub fn mode(&self, mode: u16) -> Result<bool, GhosttyError> {
        let mut query = sys::GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: the query struct has the frozen layout documented by the C
        // header and the terminal handle is valid for this call.
        check(unsafe {
            sys::ghostty_terminal_get(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_DATA_MODE,
                &mut query as *mut sys::GhosttyTerminalModeConfig as *mut c_void,
            )
        })?;
        Ok(query.value)
    }

    /// Whether any mouse-tracking mode is active. The data key returns a bool
    /// rather than a `GhosttyTerminalModeConfig` query.
    pub fn mouse_tracking(&self) -> Result<bool, GhosttyError> {
        let mut out = false;
        // SAFETY: the key's documented output type is `bool*` and the terminal
        // handle is valid for this call.
        check(unsafe {
            sys::ghostty_terminal_get(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING,
                &mut out as *mut bool as *mut c_void,
            )
        })?;
        Ok(out)
    }

    /// Select an inclusive viewport range on one row. Used by the M1 selection
    /// fixture so selection comes from Ghostty rather than a second model.
    pub fn set_row_selection(
        &mut self,
        y: u16,
        start_x: u16,
        end_x: u16,
    ) -> Result<(), GhosttyError> {
        let mut start = sys::GhosttyGridRef::default();
        let mut end = sys::GhosttyGridRef::default();
        // SAFETY: both out-parameters are valid and the points name viewport
        // cells; `grid_ref` snapshots terminal state into them.
        check(unsafe {
            sys::ghostty_terminal_grid_ref(
                self.as_raw(),
                sys::GhosttyPoint::viewport(start_x, u32::from(y)),
                &mut start,
            )
        })?;
        check(unsafe {
            sys::ghostty_terminal_grid_ref(
                self.as_raw(),
                sys::GhosttyPoint::viewport(end_x, u32::from(y)),
                &mut end,
            )
        })?;
        let selection = sys::GhosttySelection {
            start,
            end,
            rectangle: false,
        };
        // SAFETY: Ghostty copies the selection immediately, so this stack value
        // does not outlive the call; the option's value type is `GhosttySelection*`.
        check(unsafe {
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SELECTION,
                &selection as *const sys::GhosttySelection as *const c_void,
            )
        })
    }

    /// Install a viewport selection between two inclusive cell coordinates.
    /// Ghostty snapshots both endpoints as grid refs so scrollback stays valid.
    pub fn set_selection_between(
        &mut self,
        start_x: u16,
        start_y: u16,
        end_x: u16,
        end_y: u16,
    ) -> Result<(), GhosttyError> {
        let mut start = sys::GhosttyGridRef::default();
        let mut end = sys::GhosttyGridRef::default();
        // SAFETY: both out-parameters are valid and the points name viewport
        // cells; `grid_ref` snapshots terminal state into them immediately.
        check(unsafe {
            sys::ghostty_terminal_grid_ref(
                self.as_raw(),
                sys::GhosttyPoint::viewport(start_x, u32::from(start_y)),
                &mut start,
            )
        })?;
        check(unsafe {
            sys::ghostty_terminal_grid_ref(
                self.as_raw(),
                sys::GhosttyPoint::viewport(end_x, u32::from(end_y)),
                &mut end,
            )
        })?;
        let selection = sys::GhosttySelection {
            start,
            end,
            rectangle: false,
        };
        // SAFETY: Ghostty copies the selection during `set`, so this stack
        // value does not need to outlive the call.
        check(unsafe {
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SELECTION,
                &selection as *const sys::GhosttySelection as *const c_void,
            )
        })
    }

    /// Select the whole terminal buffer using Ghostty's own policy.
    pub fn select_all(&mut self) -> Result<(), GhosttyError> {
        // SAFETY: zeroed grid refs are valid POD out-values for this query.
        let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
        // SAFETY: the handle is valid and `selection` is a correctly typed out-param.
        check(unsafe { sys::ghostty_terminal_select_all(self.as_raw(), &mut selection) })?;
        // SAFETY: Ghostty copies the returned selection during `set`.
        check(unsafe {
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SELECTION,
                &selection as *const sys::GhosttySelection as *const c_void,
            )
        })
    }

    pub fn clear_selection(&mut self) -> Result<(), GhosttyError> {
        // SAFETY: NULL clears `GHOSTTY_TERMINAL_OPT_SELECTION` per the header.
        check(unsafe {
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SELECTION,
                ptr::null(),
            )
        })
    }

    /// Active Ghostty selection, if any. Used as the formatter input for copy.
    pub fn selection(&self) -> Result<Option<sys::GhosttySelection>, GhosttyError> {
        // SAFETY: zeroed grid refs are valid POD values; Ghostty overwrites
        // them on success and does not read them as inputs for this key.
        let mut raw: sys::GhosttySelection = unsafe { std::mem::zeroed() };
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_DATA_SELECTION,
                &mut raw as *mut sys::GhosttySelection as *mut c_void,
            )
        };
        match result {
            sys::GHOSTTY_SUCCESS => Ok(Some(raw)),
            sys::GHOSTTY_NO_VALUE => Ok(None),
            other => Err(GhosttyError(other)),
        }
    }

    /// Format the active selection as plain text (the same policy Ghostty uses
    /// for clipboard copy: unwrap wrapped lines, trim trailing whitespace).
    pub fn selection_text(&self) -> Result<String, GhosttyError> {
        let Some(selection) = self.selection()? else {
            return Ok(String::new());
        };
        let mut raw: sys::GhosttyFormatter = ptr::null_mut();
        // SAFETY: valid out-parameter; the formatter borrows this terminal,
        // which is owned and alive for the duration of every call below.
        check(unsafe {
            sys::ghostty_formatter_terminal_new(ptr::null(), &mut raw, self.as_raw())
        })?;
        let formatter = FormatterGuard(raw);
        let emit = sys::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        let unwrap = true;
        let trim = true;
        // SAFETY: each value has the type documented for its option and the
        // formatter handle is owned and valid.
        unsafe {
            check(sys::ghostty_formatter_set(
                formatter.0,
                sys::GHOSTTY_FORMATTER_OPT_EMIT,
                &emit as *const i32 as *const c_void,
            ))?;
            check(sys::ghostty_formatter_set(
                formatter.0,
                sys::GHOSTTY_FORMATTER_OPT_UNWRAP,
                &unwrap as *const bool as *const c_void,
            ))?;
            check(sys::ghostty_formatter_set(
                formatter.0,
                sys::GHOSTTY_FORMATTER_OPT_TRIM,
                &trim as *const bool as *const c_void,
            ))?;
            check(sys::ghostty_formatter_set(
                formatter.0,
                sys::GHOSTTY_FORMATTER_OPT_SELECTION,
                &selection as *const sys::GhosttySelection as *const c_void,
            ))?;
        }
        let mut out_ptr: *mut u8 = ptr::null_mut();
        let mut out_len = 0usize;
        // SAFETY: both out-parameters are valid and the formatter is alive;
        // the returned buffer must be freed with `ghostty_free` (done below).
        check(unsafe {
            sys::ghostty_formatter_format_alloc(
                formatter.0,
                ptr::null(),
                &mut out_ptr,
                &mut out_len,
            )
        })?;
        if out_ptr.is_null() {
            return Ok(String::new());
        }
        // SAFETY: Ghostty allocated `out_len` readable bytes at `out_ptr`.
        let bytes = unsafe { std::slice::from_raw_parts(out_ptr, out_len) };
        let text = String::from_utf8_lossy(bytes).into_owned();
        // SAFETY: `out_ptr` came from `format_alloc` with the default allocator.
        unsafe { sys::ghostty_free(out_ptr.cast()) };
        Ok(text)
    }

    /// Bound retained history deliberately. Ghostty's default byte cap is only
    /// 10 KiB, which makes even a short command lose its scrollback. A zero
    /// value disables the corresponding limit; `usize::MAX` means unlimited.
    pub fn set_scrollback_limits(&mut self, max_lines: usize, max_bytes: usize) {
        // SAFETY: both options take `size_t*`; Ghostty copies the values during
        // `set`, so these stack locals need not outlive the calls.
        unsafe {
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
                &max_lines as *const usize as *const c_void,
            );
            sys::ghostty_terminal_set(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                &max_bytes as *const usize as *const c_void,
            );
        }
    }

    /// Scroll the viewport. Ghostty owns the viewport; this only asks it to
    /// move, and render state picks up the new position on the next update.
    pub fn scroll_viewport(&mut self, behavior: sys::GhosttyTerminalScrollViewport) {
        // SAFETY: the behavior is a plain 24-byte by-value POD and the handle
        // is uniquely owned and valid for this call.
        unsafe { sys::ghostty_terminal_scroll_viewport(self.as_raw(), behavior) };
    }

    /// Scrollbar geometry in the same row space as `scroll_viewport(ROW)`.
    pub fn scrollbar(&self) -> Result<sys::GhosttyTerminalScrollbar, GhosttyError> {
        let mut raw = sys::GhosttyTerminalScrollbar::default();
        // SAFETY: the key's output type is `GhosttyTerminalScrollbar*` and the
        // handle is valid; Ghostty overwrites all three row counts.
        check(unsafe {
            sys::ghostty_terminal_get(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_DATA_SCROLLBAR,
                &mut raw as *mut sys::GhosttyTerminalScrollbar as *mut c_void,
            )
        })?;
        Ok(raw)
    }

    /// Borrowed title set by OSC 0/2, copied before the next terminal call.
    pub fn title(&self) -> String {
        let mut raw = sys::GhosttyString {
            ptr: ptr::null(),
            len: 0,
        };
        // SAFETY: the key's documented output type is `GhosttyString*`; the
        // returned bytes stay borrowed until the next mutating terminal call.
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.as_raw(),
                sys::GHOSTTY_TERMINAL_DATA_TITLE,
                &mut raw as *mut sys::GhosttyString as *mut c_void,
            )
        };
        if result != sys::GHOSTTY_SUCCESS || raw.ptr.is_null() || raw.len == 0 {
            return String::new();
        }
        // SAFETY: Ghostty reports `len` readable bytes at `ptr` for this call.
        let bytes = unsafe { std::slice::from_raw_parts(raw.ptr.cast::<u8>(), raw.len) };
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Install the shared effect callbacks. The returned token must be dropped
    /// only after this terminal has been freed.
    pub fn install_callbacks(
        &mut self,
        writer: Arc<WriterQueue>,
        on_title: Box<dyn Fn(String) + Send + Sync>,
    ) -> Result<CallbackRegistration, GhosttyError> {
        let userdata = Arc::into_raw(Arc::new(CallbackData {
            writer,
            on_title,
            color_scheme: Arc::clone(&self.color_scheme),
        })) as *mut c_void;
        // SAFETY: `userdata` is a strong Arc reference kept alive by the token
        // and by the terminal option until both are torn down.
        let result = unsafe {
            sys::ghostty_terminal_set(self.as_raw(), sys::GHOSTTY_TERMINAL_OPT_USERDATA, userdata)
        };
        if let Err(error) = check(result) {
            // SAFETY: reclaim the reference because installation failed.
            unsafe { drop(Arc::from_raw(userdata.cast::<CallbackData>())) };
            return Err(error);
        }
        // SAFETY: each function pointer matches the callback signature Ghostty
        // documents for its option; Ghostty copies the pointers.
        for (option, callback) in [
            (
                sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                write_pty_callback as *const c_void,
            ),
            (
                sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                title_changed_callback as *const c_void,
            ),
            (
                sys::GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
                color_scheme_callback as *const c_void,
            ),
        ] {
            let result = unsafe { sys::ghostty_terminal_set(self.as_raw(), option, callback) };
            if let Err(error) = check(result) {
                // SAFETY: clear userdata so Ghostty holds no stale pointer.
                unsafe {
                    sys::ghostty_terminal_set(
                        self.as_raw(),
                        sys::GHOSTTY_TERMINAL_OPT_USERDATA,
                        ptr::null(),
                    )
                };
                unsafe { drop(Arc::from_raw(userdata.cast::<CallbackData>())) };
                return Err(error);
            }
        }
        Ok(CallbackRegistration { userdata })
    }
}

/// RAII guard so an error during formatter setup still frees the handle.
struct FormatterGuard(sys::GhosttyFormatter);

impl Drop for FormatterGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from `formatter_terminal_new` and is freed once.
        unsafe { sys::ghostty_formatter_free(self.0) };
    }
}

/// Data shared with Ghostty's effect callbacks through `OPT_USERDATA`.
/// Callbacks must stay non-blocking and must not re-enter terminal writes.
struct CallbackData {
    writer: Arc<WriterQueue>,
    on_title: Box<dyn Fn(String) + Send + Sync>,
    color_scheme: Arc<AtomicU8>,
}

/// `GhosttyTerminalColorSchemeFn`: answer `CSI ? 996 n` with the palette's
/// appearance. The shared atomic is written only under the terminal lock.
unsafe extern "C" fn color_scheme_callback(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut i32,
) -> bool {
    if userdata.is_null() || out_scheme.is_null() {
        return false;
    }
    // SAFETY: same ownership invariant as `write_pty_callback`.
    let callbacks = unsafe { &*(userdata.cast::<CallbackData>()) };
    let scheme = callbacks.color_scheme.load(Ordering::Acquire);
    // SAFETY: `out_scheme` is the typed out-parameter Ghostty supplies, and
    // `GhosttyColorScheme` is a C enum backed by int.
    unsafe {
        *out_scheme = if scheme == sys::GHOSTTY_COLOR_SCHEME_DARK as u8 {
            sys::GHOSTTY_COLOR_SCHEME_DARK
        } else {
            sys::GHOSTTY_COLOR_SCHEME_LIGHT
        };
    }
    true
}

/// `GhosttyTerminalWritePtyFn`: copy the response into the writer queue and
/// return. No terminal calls, no OS I/O, no blocking — avoids lock inversion.
unsafe extern "C" fn write_pty_callback(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || data.is_null() {
        return;
    }
    // SAFETY: userdata points at an `Arc<CallbackData>` kept alive by the
    // registration token for as long as the terminal can invoke this callback.
    let callbacks = unsafe { &*(userdata.cast::<CallbackData>()) };
    // SAFETY: Ghostty guarantees `data` is readable for `len` bytes this call.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    callbacks.writer.push(bytes);
}

/// `GhosttyTerminalTitleChangedFn`: copy the borrowed title and hand it to the
/// sink. The sink only posts to the UI; it must not call back into Ghostty.
unsafe extern "C" fn title_changed_callback(terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() || terminal.is_null() {
        return;
    }
    // SAFETY: same ownership invariant as `write_pty_callback`.
    let callbacks = unsafe { &*(userdata.cast::<CallbackData>()) };
    let mut raw = sys::GhosttyString {
        ptr: ptr::null(),
        len: 0,
    };
    // SAFETY: the header explicitly directs callbacks to read the new title
    // with `GHOSTTY_TERMINAL_DATA_TITLE`; `raw` is a typed out-parameter.
    let result = unsafe {
        sys::ghostty_terminal_get(
            terminal,
            sys::GHOSTTY_TERMINAL_DATA_TITLE,
            &mut raw as *mut sys::GhosttyString as *mut c_void,
        )
    };
    if result != sys::GHOSTTY_SUCCESS || raw.ptr.is_null() {
        return;
    }
    // SAFETY: the borrowed string is valid for this callback; copy it before
    // any other terminal call can invalidate the pointer.
    let bytes = unsafe { std::slice::from_raw_parts(raw.ptr.cast::<u8>(), raw.len) };
    (callbacks.on_title)(String::from_utf8_lossy(bytes).into_owned());
}

/// Holds the strong Arc reference leaked to Ghostty as callback userdata.
pub struct CallbackRegistration {
    userdata: *mut c_void,
}

// SAFETY: the userdata Arc is owned solely by this token after Ghostty is freed.
unsafe impl Send for CallbackRegistration {}

impl Drop for CallbackRegistration {
    fn drop(&mut self) {
        // SAFETY: `userdata` was produced by `Arc::into_raw` in
        // `install_callbacks` and is reclaimed exactly once here.
        unsafe { drop(Arc::from_raw(self.userdata.cast::<CallbackData>())) };
    }
}

// SAFETY: a terminal handle has no thread affinity, but Ghostty's terminal
// operations are not mutually thread-safe. `TerminalSession` is the single
// serialization point: it owns this value behind one mutex and never exposes
// the raw handle. Do not construct a second owner or share it without a lock.
unsafe impl Send for GhosttyTerminal {}

impl Drop for GhosttyTerminal {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` guarantees this handle is freed at most once and
            // the wrapper is consumed, so no live reference can use it after.
            unsafe { sys::ghostty_terminal_free(raw) };
        }
    }
}

/// Owns one `GhosttyRenderState` handle.
pub struct GhosttyRenderState {
    raw: Option<sys::GhosttyRenderState>,
}

impl GhosttyRenderState {
    pub fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyRenderState = ptr::null_mut();
        // SAFETY: `raw` is a valid out-parameter, NULL selects the default
        // allocator, and the handle is owned exclusively by this wrapper.
        check(unsafe { sys::ghostty_render_state_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyRenderState {
        self.raw
            .expect("owned render-state handle is still present")
    }

    /// Phase one: copy terminal data into render state. This is the only
    /// render call that needs the terminal lock.
    pub fn begin_update(&mut self, terminal: &GhosttyTerminal) -> Result<(), GhosttyError> {
        // SAFETY: both handles are non-null, uniquely owned and alive; the
        // begin phase borrows the terminal immutably for its duration.
        check(unsafe { sys::ghostty_render_state_begin_update(self.as_raw(), terminal.as_raw()) })
    }

    /// Phase two: finalize render-state memory without touching the terminal.
    /// The caller must have completed a matching `begin_update` first.
    pub fn end_update(&mut self) -> Result<(), GhosttyError> {
        // SAFETY: a successful begin guarantees end has a complete update to
        // finish; it touches only render-state-owned memory.
        check(unsafe { sys::ghostty_render_state_end_update(self.as_raw()) })
    }

    /// Two-phase update: the begin call is the only one that needs the
    /// terminal, which is why M2 can hold its lock just around this method.
    pub fn update(&mut self, terminal: &GhosttyTerminal) -> Result<(), GhosttyError> {
        self.begin_update(terminal)?;
        self.end_update()
    }

    /// Rows Ghostty reports as dirty in the current update, in viewport
    /// coordinates. Used by M2's render counters.
    pub fn dirty_rows(&self) -> Result<Vec<u16>, GhosttyError> {
        let mut rows = Vec::new();
        let mut iter = RowIterator::new(self)?;
        let mut y = 0u16;
        while iter.next_dirty(&mut y) {
            rows.push(y);
        }
        Ok(rows)
    }

    /// Mark every dirty flag consumed. Call only after a complete frame was
    /// read; a failed frame must leave the next update able to rebuild.
    pub fn clean(&mut self) -> Result<(), GhosttyError> {
        // SAFETY: the handle is uniquely owned and valid for this call.
        check(unsafe { sys::ghostty_render_state_clean(self.as_raw()) })
    }

    fn get_u16(&self, data: i32) -> Result<u16, GhosttyError> {
        let mut out = 0u16;
        // SAFETY: `out` has the type documented for these data keys and the
        // render-state handle is valid.
        check(unsafe {
            sys::ghostty_render_state_get(self.as_raw(), data, &mut out as *mut u16 as *mut c_void)
        })?;
        Ok(out)
    }

    fn get_i32(&self, data: i32) -> Result<i32, GhosttyError> {
        let mut out = 0i32;
        // SAFETY: `out` has the enum-sized type used by dirty state and the
        // render-state handle is valid.
        check(unsafe {
            sys::ghostty_render_state_get(self.as_raw(), data, &mut out as *mut i32 as *mut c_void)
        })?;
        Ok(out)
    }

    pub fn cursor(&self) -> Result<Cursor, GhosttyError> {
        let mut raw = sys::GhosttyRenderStateCursor::sized();
        // SAFETY: `sized()` sets the struct's size field as the C API requires,
        // and the render-state handle is valid.
        check(unsafe {
            sys::ghostty_render_state_get(
                self.as_raw(),
                sys::GHOSTTY_RENDER_STATE_DATA_CURSOR,
                &mut raw as *mut sys::GhosttyRenderStateCursor as *mut c_void,
            )
        })?;
        Ok(Cursor {
            has_value: raw.viewport_has_value,
            x: raw.viewport_x,
            y: raw.viewport_y,
            wide_tail: raw.wide_tail,
            visible: raw.visible,
            blinking: raw.blinking,
            password_input: raw.password_input,
            visual_style: raw.visual_style,
        })
    }

    /// Read the resolved palette once per frame instead of per cell.
    pub fn colors(&self) -> Result<Colors, GhosttyError> {
        let mut raw = sys::GhosttyRenderStateColors::sized();
        // SAFETY: `sized()` sets the required size field and the handle is
        // valid; the C API only writes the documented fields.
        check(unsafe {
            sys::ghostty_render_state_get(
                self.as_raw(),
                sys::GHOSTTY_RENDER_STATE_DATA_COLORS,
                &mut raw as *mut sys::GhosttyRenderStateColors as *mut c_void,
            )
        })?;
        let rgb = |c: sys::GhosttyColorRgb| (c.r, c.g, c.b);
        Ok(Colors {
            background: rgb(raw.background),
            foreground: rgb(raw.foreground),
            cursor: rgb(raw.cursor),
            cursor_has_value: raw.cursor_has_value,
            palette: raw.palette.map(rgb),
        })
    }

    fn populate_row_iterator(
        &self,
        iterator: *mut sys::GhosttyRenderStateRowIterator,
    ) -> Result<(), GhosttyError> {
        // SAFETY: the render state is borrowed immutably for the iterator's
        // lifetime and `iterator` points at the handle allocated by
        // `RowIterator::new`, which has the type Ghostty documents for this key.
        check(unsafe {
            sys::ghostty_render_state_get(
                self.as_raw(),
                sys::GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR,
                iterator.cast(),
            )
        })
    }

    /// Copy the entire visible grid into Rust-owned values. Do not call
    /// `clean` until the returned snapshot has been consumed successfully.
    pub fn snapshot(&self) -> Result<RenderSnapshot, GhosttyError> {
        let cols = self.get_u16(sys::GHOSTTY_RENDER_STATE_DATA_COLS)?;
        let rows = self.get_u16(sys::GHOSTTY_RENDER_STATE_DATA_ROWS)?;
        let dirty = self.get_i32(sys::GHOSTTY_RENDER_STATE_DATA_DIRTY)?;
        let cursor = self.cursor()?;
        let colors = self.colors()?;

        let mut lines = vec![Vec::new(); usize::from(rows)];
        let mut selections = vec![None; usize::from(rows)];
        let dirty_rows = self.dirty_rows()?;
        let mut row_iter = RowIterator::new(self)?;
        let mut cells = RowCells::new()?;
        while row_iter.next() {
            let y = row_iter.viewport_y()?;
            // Query selection once per row: the per-cell API exists but costs
            // one FFI call per cell, which the render policy avoids.
            let selection = row_iter.selection()?;
            cells.bind(&mut row_iter)?;
            let mut line = Vec::new();
            while cells.next() {
                let mut cell = cells.cell()?;
                if let Some(range) = selection {
                    cell.selected = range.contains(u16::try_from(line.len()).unwrap_or(u16::MAX));
                }
                line.push(cell);
            }
            if let Some(slot) = lines.get_mut(usize::from(y)) {
                *slot = line;
            }
            if let Some(slot) = selections.get_mut(usize::from(y)) {
                *slot = selection;
            }
        }
        Ok(RenderSnapshot {
            cols,
            rows,
            dirty,
            cursor,
            colors,
            lines,
            selections,
            dirty_rows,
        })
    }
}

impl Default for GhosttyRenderState {
    fn default() -> Self {
        Self::new().expect("default render state allocation succeeded")
    }
}

// SAFETY: render-state handles have no thread affinity. `TerminalSession`
// serializes all access behind one mutex; the handle is never shared directly.
unsafe impl Send for GhosttyRenderState {}

impl Drop for GhosttyRenderState {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_render_state_free(raw) };
        }
    }
}

/// Owns one row-iterator handle borrowed from a render state.
pub struct RowIterator<'a> {
    raw: Option<sys::GhosttyRenderStateRowIterator>,
    _state: PhantomData<&'a GhosttyRenderState>,
}

impl<'a> RowIterator<'a> {
    fn new(state: &'a GhosttyRenderState) -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyRenderStateRowIterator = ptr::null_mut();
        // SAFETY: `raw` is a valid out-parameter and NULL selects the default
        // allocator; the handle is owned by this iterator alone.
        check(unsafe { sys::ghostty_render_state_row_iterator_new(ptr::null(), &mut raw) })?;
        state.populate_row_iterator(&mut raw)?;
        Ok(Self {
            raw: Some(raw),
            _state: PhantomData,
        })
    }

    fn as_raw(&self) -> sys::GhosttyRenderStateRowIterator {
        self.raw
            .expect("owned row iterator handle is still present")
    }

    // This mirrors Ghostty's C iterator handle, whose `next` advances a cursor
    // and returns whether a row is available; it is not an Iterator<Item>.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn next(&mut self) -> bool {
        // SAFETY: the handle is non-null, valid and uniquely borrowed here.
        unsafe { sys::ghostty_render_state_row_iterator_next(self.as_raw()) }
    }

    #[must_use]
    pub fn next_dirty(&mut self, y: &mut u16) -> bool {
        // SAFETY: the handle is valid and `y` is a valid `uint16_t` out-param.
        unsafe { sys::ghostty_render_state_row_iterator_next_dirty(self.as_raw(), y) }
    }

    fn get_i32(&self, data: i32) -> Result<i32, GhosttyError> {
        let mut out = 0i32;
        // SAFETY: `out` has the type documented for row data keys.
        check(unsafe {
            sys::ghostty_render_state_row_get(
                self.as_raw(),
                data,
                &mut out as *mut i32 as *mut c_void,
            )
        })?;
        Ok(out)
    }

    /// Row position relative to the top of the viewport. Ghostty reports it
    /// as `int32_t` because overscan rows can be negative.
    pub fn viewport_y(&self) -> Result<u16, GhosttyError> {
        let y = self.get_i32(sys::GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y)?;
        u16::try_from(y).map_err(|_| GhosttyError(sys::GHOSTTY_INVALID_VALUE))
    }

    /// The row-local selection range. `GHOSTTY_NO_VALUE` means no selection:
    /// callers should not issue a per-cell selection query in that case.
    pub fn selection(&self) -> Result<Option<RowSelection>, GhosttyError> {
        let mut raw = sys::GhosttyRenderStateRowSelection::sized();
        // SAFETY: `sized()` fills the required size field and the row iterator
        // is positioned on a live row from the current render-state update.
        let result = unsafe {
            sys::ghostty_render_state_row_get(
                self.as_raw(),
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                &mut raw as *mut sys::GhosttyRenderStateRowSelection as *mut c_void,
            )
        };
        match result {
            sys::GHOSTTY_SUCCESS => Ok(Some(RowSelection {
                start_x: raw.start_x,
                end_x: raw.end_x,
            })),
            sys::GHOSTTY_NO_VALUE => Ok(None),
            other => Err(GhosttyError(other)),
        }
    }
}

impl Drop for RowIterator<'_> {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_render_state_row_iterator_free(raw) };
        }
    }
}

/// Owns one reusable row-cells handle. It is populated from whichever row the
/// passed iterator currently points at, so one handle serves a whole frame.
pub struct RowCells {
    raw: Option<sys::GhosttyRenderStateRowCells>,
}

impl RowCells {
    fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyRenderStateRowCells = ptr::null_mut();
        // SAFETY: `raw` is a valid out-parameter and NULL selects the default
        // allocator; the handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_render_state_row_cells_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyRenderStateRowCells {
        self.raw.expect("owned row-cells handle is still present")
    }

    fn bind(&mut self, row: &mut RowIterator<'_>) -> Result<(), GhosttyError> {
        // The out-parameter is a pointer to the cells handle, like the
        // Ghostty C render example (`&cells`), not the handle value itself.
        let mut handle = self.as_raw();
        // SAFETY: the row iterator is positioned on a row, `handle` points at
        // the handle we own, and row memory stays valid until the next update.
        check(unsafe {
            sys::ghostty_render_state_row_get(
                row.as_raw(),
                sys::GHOSTTY_RENDER_STATE_ROW_DATA_CELLS,
                &mut handle as *mut sys::GhosttyRenderStateRowCells as *mut c_void,
            )
        })?;
        self.raw = Some(handle);
        Ok(())
    }

    // Same Ghostty C iterator shape as the row iterator above, not Iterator.
    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn next(&mut self) -> bool {
        // SAFETY: the handle is valid and uniquely borrowed here.
        unsafe { sys::ghostty_render_state_row_cells_next(self.as_raw()) }
    }

    #[allow(dead_code)]
    pub fn select(&mut self, x: u16) -> Result<(), GhosttyError> {
        // SAFETY: the handle is valid and `x` is the documented column type.
        check(unsafe { sys::ghostty_render_state_row_cells_select(self.as_raw(), x) })
    }

    fn get<T>(&self, data: i32, out: &mut T) -> Result<sys::GhosttyResult, GhosttyError> {
        // SAFETY: callers only pass types documented for the requested data
        // key, and both handle and out pointer are valid for this call.
        let result = unsafe {
            sys::ghostty_render_state_row_cells_get(
                self.as_raw(),
                data,
                out as *mut T as *mut c_void,
            )
        };
        // OUT_OF_SPACE is a valid answer for the UTF-8 sizing probe; callers
        // decide whether it is acceptable for their data key.
        if result == sys::GHOSTTY_SUCCESS
            || result == sys::GHOSTTY_NO_VALUE
            || result == sys::GHOSTTY_OUT_OF_SPACE
        {
            Ok(result)
        } else {
            Err(GhosttyError(result))
        }
    }

    fn cell_color(&self, data: i32) -> Result<Option<(u8, u8, u8)>, GhosttyError> {
        let mut color = sys::GhosttyColorRgb::default();
        if self.get(data, &mut color)? == sys::GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        Ok(Some((color.r, color.g, color.b)))
    }

    /// Read the current cell as plain Rust values.
    pub fn cell(&self) -> Result<Cell, GhosttyError> {
        let mut codepoints_len = 0u32;
        self.get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN,
            &mut codepoints_len,
        )?;

        // A NULL buffer asks Ghostty for the required byte count.
        let mut probe = sys::GhosttyBuffer {
            ptr: ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let probed = self.get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
            &mut probe,
        )?;
        let utf8_len = probe.len;
        let out_of_space = probed == sys::GHOSTTY_OUT_OF_SPACE;

        let mut text = Vec::new();
        if utf8_len > 0 {
            text.resize(utf8_len, 0u8);
            let mut buffer = sys::GhosttyBuffer {
                ptr: text.as_mut_ptr(),
                cap: text.len(),
                len: 0,
            };
            check(self.get(
                sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,
                &mut buffer,
            )?)?;
            text.truncate(buffer.len);
        } else if !out_of_space && codepoints_len == 0 {
            text.clear();
        }

        let mut raw_style = sys::GhosttyStyle {
            fg_color: sys::GhosttyStyleColor {
                tag: sys::GHOSTTY_STYLE_COLOR_NONE,
                value: sys::GhosttyStyleColorValue {
                    rgb: sys::GhosttyColorRgb::default(),
                },
            },
            bg_color: sys::GhosttyStyleColor {
                tag: sys::GHOSTTY_STYLE_COLOR_NONE,
                value: sys::GhosttyStyleColorValue {
                    rgb: sys::GhosttyColorRgb::default(),
                },
            },
            underline_color: sys::GhosttyStyleColor {
                tag: sys::GHOSTTY_STYLE_COLOR_NONE,
                value: sys::GhosttyStyleColorValue {
                    rgb: sys::GhosttyColorRgb::default(),
                },
            },
            flags: 0,
            reserved: 0,
        };
        // SAFETY: the locals above are writable `GhosttyStyle` values and the
        // C function only writes the documented sized fields.
        unsafe { sys::ghostty_style_default(&mut raw_style) };
        let _ = self.get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
            &mut raw_style,
        )?;

        // Selection is resolved once per row by `RowIterator::selection`;
        // `snapshot` fills this field from that range.
        let selected = false;
        let mut has_styling = false;
        self.get(
            sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
            &mut has_styling,
        )?;

        Ok(Cell {
            text: String::from_utf8_lossy(&text).into_owned(),
            style: Style::from_raw(raw_style),
            foreground: self.cell_color(sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR)?,
            background: self.cell_color(sys::GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR)?,
            selected,
            has_styling,
        })
    }
}

impl Drop for RowCells {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_render_state_row_cells_free(raw) };
        }
    }
}

/// Owns one key encoder. Sync it from terminal modes before encoding.
pub struct KeyEncoder {
    raw: Option<sys::GhosttyKeyEncoder>,
}

impl KeyEncoder {
    pub fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyKeyEncoder = ptr::null_mut();
        // SAFETY: valid out-parameter, NULL selects the default allocator,
        // handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_key_encoder_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyKeyEncoder {
        self.raw.expect("owned key encoder handle is still present")
    }

    pub fn sync_from_terminal(&mut self, terminal: &GhosttyTerminal) -> Result<(), GhosttyError> {
        // SAFETY: both handles are valid and the terminal is borrowed without
        // mutation for the duration of the call.
        check(unsafe {
            sys::ghostty_key_encoder_set_from_terminal(self.as_raw(), terminal.as_raw())
        })
    }

    fn encode_into(&mut self, event: &KeyEvent, buf: &mut [u8]) -> (sys::GhosttyResult, usize) {
        let mut written = 0usize;
        // SAFETY: both handles are valid; `buf` stays alive and is writable
        // for the call, and `written` is a valid out-parameter. On
        // `out_of_space`, Ghostty still reports the required length here.
        let result = unsafe {
            sys::ghostty_key_encoder_encode(
                self.as_raw(),
                event.as_raw(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            )
        };
        (result, written)
    }

    /// Encode a key event into `buf`; returns the number of bytes written.
    pub fn encode(&mut self, event: &KeyEvent, buf: &mut [u8]) -> Result<usize, GhosttyError> {
        let (result, written) = self.encode_into(event, buf);
        check(result)?;
        Ok(written)
    }

    /// Encode into an owned buffer, growing it once if the fixed-size buffer
    /// is too small. A dropped key is worse than one extra allocation, and
    /// composed text with kitty's `report_associated` can exceed any fixed
    /// size.
    pub fn encode_owned(&mut self, event: &KeyEvent) -> Result<Vec<u8>, GhosttyError> {
        let mut buf = [0u8; 256];
        let (result, written) = self.encode_into(event, &mut buf);
        if result == sys::GHOSTTY_SUCCESS {
            return Ok(buf[..written].to_vec());
        }
        if result != sys::GHOSTTY_OUT_OF_SPACE {
            return Err(GhosttyError(result));
        }
        // `written` is now the required length reported by Ghostty.
        let mut dynamic = vec![0u8; written];
        let (result, actual) = self.encode_into(event, &mut dynamic);
        check(result)?;
        dynamic.truncate(actual);
        Ok(dynamic)
    }
}

// SAFETY: the encoder has no thread affinity; `TerminalSession` serializes
// every call behind one mutex and never exposes the raw handle.
unsafe impl Send for KeyEncoder {}

impl Default for KeyEncoder {
    fn default() -> Self {
        Self::new().expect("default key encoder allocation succeeded")
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_key_encoder_free(raw) };
        }
    }
}

/// Owns one key event used as input to `KeyEncoder::encode`.
pub struct KeyEvent {
    raw: Option<sys::GhosttyKeyEvent>,
}

impl KeyEvent {
    pub fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyKeyEvent = ptr::null_mut();
        // SAFETY: valid out-parameter, NULL selects the default allocator,
        // handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_key_event_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyKeyEvent {
        self.raw.expect("owned key event handle is still present")
    }

    pub fn press(&mut self, key: i32, mods: u16) -> Result<(), GhosttyError> {
        self.set_action_key_mods(sys::GHOSTTY_KEY_ACTION_PRESS, key, mods)
    }

    /// A held key is a repeat, not a fresh press. Kitty's `report_events` mode
    /// distinguishes them, so preserve the platform's `is_held` flag.
    pub fn repeat(&mut self, key: i32, mods: u16) -> Result<(), GhosttyError> {
        self.set_action_key_mods(sys::GHOSTTY_KEY_ACTION_REPEAT, key, mods)
    }

    fn set_action_key_mods(
        &mut self,
        action: i32,
        key: i32,
        mods: u16,
    ) -> Result<(), GhosttyError> {
        // SAFETY: the handle is valid and these values are the documented
        // enum/bitmask representations of a key action.
        check(unsafe { sys::ghostty_key_event_set_action(self.as_raw(), action) })?;
        check(unsafe { sys::ghostty_key_event_set_key(self.as_raw(), key) })?;
        check(unsafe { sys::ghostty_key_event_set_mods(self.as_raw(), mods) })
    }

    /// The physical key's unshifted Unicode codepoint. Independent from the
    /// committed UTF-8 text: a shifted `A` commits "A" but its unshifted
    /// codepoint is 'a'. Ghostty needs this for CSI-u and modifyOtherKeys.
    pub fn set_unshifted_codepoint(&mut self, codepoint: u32) -> Result<(), GhosttyError> {
        // SAFETY: the handle is valid and the codepoint is a Unicode scalar.
        check(unsafe { sys::ghostty_key_event_set_unshifted_codepoint(self.as_raw(), codepoint) })
    }

    pub fn set_utf8(&mut self, text: &str) -> Result<(), GhosttyError> {
        let string = sys::GhosttyString {
            ptr: text.as_ptr(),
            len: text.len(),
        };
        // SAFETY: `text` outlives the call and Ghostty copies the borrowed
        // string; the event handle is valid.
        check(unsafe { sys::ghostty_key_event_set_utf8(self.as_raw(), string) })?;
        if let Some(first) = text.chars().next() {
            // SAFETY: the event handle is valid and a Unicode scalar is the
            // documented unshifted-codepoint representation.
            check(unsafe {
                sys::ghostty_key_event_set_unshifted_codepoint(self.as_raw(), u32::from(first))
            })?;
        }
        Ok(())
    }
}

impl Default for KeyEvent {
    fn default() -> Self {
        Self::new().expect("default key event allocation succeeded")
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_key_event_free(raw) };
        }
    }
}

/// Conservative pre-check for text that could inject shell commands.
#[must_use]
pub fn paste_is_safe(text: &str) -> bool {
    // SAFETY: `text` outlives the call and the C function only reads `len`
    // bytes; it does not require NUL termination.
    unsafe { sys::ghostty_paste_is_safe(text.as_ptr().cast(), text.len()) }
}

/// Encode clipboard text for terminal input, with bracketed-paste framing
/// when the terminal has that mode enabled.
pub fn paste_encode(text: &str, bracketed: bool) -> Result<Vec<u8>, GhosttyError> {
    // The C encoder deliberately mutates its input buffer, so work on a copy.
    let mut data = text.as_bytes().to_vec();
    let mut needed = 0usize;
    // SAFETY: `data` is writable and owned here; a NULL output buffer with
    // zero capacity means "report required size"; `needed` is a valid out-param.
    let probe = unsafe {
        sys::ghostty_paste_encode(
            data.as_mut_ptr().cast(),
            data.len(),
            bracketed,
            ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    if probe == sys::GHOSTTY_SUCCESS {
        return Ok(Vec::new());
    }
    if probe != sys::GHOSTTY_OUT_OF_SPACE {
        return Err(GhosttyError(probe));
    }

    let mut output = vec![0u8; needed];
    let mut written = 0usize;
    // SAFETY: `output` is writable with the capacity just reported, `data` is
    // still owned here, and `written` is a valid out-parameter.
    check(unsafe {
        sys::ghostty_paste_encode(
            data.as_mut_ptr().cast(),
            data.len(),
            bracketed,
            output.as_mut_ptr(),
            output.len(),
            &mut written,
        )
    })?;
    output.truncate(written);
    Ok(output)
}

/// Owns one `GhosttyMouseEncoder`. Synced from terminal tracking modes before
/// each event; the terminal decides whether to emit X10/SGR/UTF-8 sequences.
pub struct MouseEncoder {
    raw: Option<sys::GhosttyMouseEncoder>,
}

impl MouseEncoder {
    pub fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyMouseEncoder = ptr::null_mut();
        // SAFETY: valid out-parameter, NULL selects the default allocator, and
        // the handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_mouse_encoder_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyMouseEncoder {
        self.raw
            .expect("owned mouse encoder handle is still present")
    }

    pub fn sync_from_terminal(&mut self, terminal: &GhosttyTerminal) -> Result<(), GhosttyError> {
        // SAFETY: both handles are valid and the terminal is not mutated here.
        check(unsafe {
            sys::ghostty_mouse_encoder_set_from_terminal(self.as_raw(), terminal.as_raw())
        })
    }

    pub fn set_size(&mut self, size: &sys::GhosttyMouseEncoderSize) -> Result<(), GhosttyError> {
        // SAFETY: `size` is a correctly sized struct; Ghostty copies it.
        check(unsafe {
            sys::ghostty_mouse_encoder_set(
                self.as_raw(),
                sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                size as *const sys::GhosttyMouseEncoderSize as *const c_void,
            )
        })
    }

    pub fn track_last_cell(&mut self, enabled: bool) -> Result<(), GhosttyError> {
        // SAFETY: the option's value type is `bool`; Ghostty copies it.
        check(unsafe {
            sys::ghostty_mouse_encoder_set(
                self.as_raw(),
                sys::GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                &enabled as *const bool as *const c_void,
            )
        })
    }

    /// Reflect whether a button is held. Ghostty uses this to keep reporting a
    /// drag after the pointer leaves the terminal surface; without it those
    /// motion events are silently dropped.
    pub fn set_any_button_pressed(&mut self, pressed: bool) -> Result<(), GhosttyError> {
        // SAFETY: the option's value type is `bool`; Ghostty copies it.
        check(unsafe {
            sys::ghostty_mouse_encoder_set(
                self.as_raw(),
                sys::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                &pressed as *const bool as *const c_void,
            )
        })
    }

    /// Encode an event. Returns zero bytes when tracking is off or deduplicated.
    pub fn encode(&mut self, event: &MouseEvent, buf: &mut [u8]) -> Result<usize, GhosttyError> {
        let mut written = 0usize;
        // SAFETY: both handles are valid, `buf` is writable for the call, and
        // `written` is a valid out-parameter.
        check(unsafe {
            sys::ghostty_mouse_encoder_encode(
                self.as_raw(),
                event.as_raw(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            )
        })?;
        Ok(written)
    }
}

// SAFETY: the encoder has no thread affinity; `TerminalSession` serializes all
// access behind one mutex.
unsafe impl Send for MouseEncoder {}

impl Default for MouseEncoder {
    fn default() -> Self {
        Self::new().expect("default mouse encoder allocation succeeded")
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_mouse_encoder_free(raw) };
        }
    }
}

/// Owns one `GhosttyMouseEvent`, reused across pointer events.
pub struct MouseEvent {
    raw: Option<sys::GhosttyMouseEvent>,
}

impl MouseEvent {
    pub fn new() -> Result<Self, GhosttyError> {
        let mut raw: sys::GhosttyMouseEvent = ptr::null_mut();
        // SAFETY: valid out-parameter, NULL selects the default allocator, and
        // the handle is owned by this wrapper alone.
        check(unsafe { sys::ghostty_mouse_event_new(ptr::null(), &mut raw) })?;
        Ok(Self { raw: Some(raw) })
    }

    fn as_raw(&self) -> sys::GhosttyMouseEvent {
        self.raw.expect("owned mouse event handle is still present")
    }

    pub fn configure(
        &mut self,
        action: i32,
        button: Option<i32>,
        mods: u16,
        x: f32,
        y: f32,
    ) -> Result<(), GhosttyError> {
        // SAFETY: all values are documented enum/bitmask representations and
        // the handle is valid for these setters.
        check(unsafe { sys::ghostty_mouse_event_set_action(self.as_raw(), action) })?;
        match button {
            Some(button) => {
                check(unsafe { sys::ghostty_mouse_event_set_button(self.as_raw(), button) })?;
            }
            None => {
                check(unsafe { sys::ghostty_mouse_event_clear_button(self.as_raw()) })?;
            }
        }
        check(unsafe { sys::ghostty_mouse_event_set_mods(self.as_raw(), mods) })?;
        check(unsafe {
            sys::ghostty_mouse_event_set_position(self.as_raw(), sys::GhosttyMousePosition { x, y })
        })
    }
}

// SAFETY: the event has no thread affinity; `TerminalSession` serializes all
// access behind one mutex and never exposes the raw handle.
unsafe impl Send for MouseEvent {}

impl Default for MouseEvent {
    fn default() -> Self {
        Self::new().expect("default mouse event allocation succeeded")
    }
}

impl Drop for MouseEvent {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY: `take` makes this the single free of the owned handle.
            unsafe { sys::ghostty_mouse_event_free(raw) };
        }
    }
}
