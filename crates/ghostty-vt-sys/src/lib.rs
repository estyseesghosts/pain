//! Raw FFI declarations for libghostty-vt 1.0.2.
//!
//! These declarations are checked in, not generated at build time, so the ABI
//! surface is small and auditable. They mirror the headers in
//! `vendor/ghostty-vt/include/ghostty/` at tag `vt-1.0.2`. Do not add a
//! declaration from a different Ghostty revision without moving the whole pin.
//!
//! The crate builds and links the static library in `build.rs`; consumers only
//! need to link this crate.

#![allow(non_camel_case_types, non_upper_case_globals)]

use core::ffi::{c_char, c_void};

/// `GhosttyResult` is a C enum backed by `int`.
pub type GhosttyResult = i32;

pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;
pub const GHOSTTY_IO_ERROR: GhosttyResult = -5;
pub const GHOSTTY_LIMIT_EXCEEDED: GhosttyResult = -6;
pub const GHOSTTY_REJECTED: GhosttyResult = -7;
pub const GHOSTTY_UNSUPPORTED: GhosttyResult = -8;

/// Opaque handles. The C API exposes them as pointers to incomplete structs.
pub type GhosttyTerminal = *mut c_void;
pub type GhosttyRenderState = *mut c_void;
pub type GhosttyRenderStateRowIterator = *mut c_void;
pub type GhosttyRenderStateRowCells = *mut c_void;
pub type GhosttyKeyEncoder = *mut c_void;
pub type GhosttyKeyEvent = *mut c_void;
pub type GhosttyMouseEncoder = *mut c_void;
pub type GhosttyMouseEvent = *mut c_void;
pub type GhosttyFormatter = *mut c_void;

// `ghostty/vt/terminal.h`: options and data used by the MVP effects.
pub const GHOSTTY_TERMINAL_OPT_USERDATA: i32 = 1;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: i32 = 2;
pub const GHOSTTY_TERMINAL_OPT_BELL: i32 = 3;
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: i32 = 6;
pub const GHOSTTY_TERMINAL_OPT_SIZE: i32 = 7;
pub const GHOSTTY_TERMINAL_OPT_TITLE: i32 = 10;
// Color scheme query callback (`CSI ? 996 n`) and default terminal colors.
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: i32 = 8;
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: i32 = 12;
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: i32 = 13;
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: i32 = 14;
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: i32 = 15;
// `GhosttyColorScheme` is a C enum backed by int.
pub const GHOSTTY_COLOR_SCHEME_LIGHT: i32 = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: i32 = 1;
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: i32 = 26;
pub const GHOSTTY_TERMINAL_OPT_SELECTION: i32 = 22;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE: i32 = 23;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK: i32 = 24;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: i32 = 28;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: i32 = 29;
pub const GHOSTTY_TERMINAL_OPT_MODE_DEFAULT: i32 = 34;
pub const GHOSTTY_TERMINAL_OPT_MODE: i32 = 35;

// Scroll viewport tags (`ghostty/vt/terminal.h`).
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP: i32 = 0;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM: i32 = 1;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA: i32 = 2;
pub const GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW: i32 = 3;

pub const GHOSTTY_TERMINAL_DATA_COLS: i32 = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: i32 = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: i32 = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: i32 = 4;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP: i32 = 5;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: i32 = 6;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: i32 = 7;
pub const GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS: i32 = 8;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBAR: i32 = 9;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_SGR_STYLE: i32 = 10;
pub const GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING: i32 = 11;
pub const GHOSTTY_TERMINAL_DATA_TITLE: i32 = 12;
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: i32 = 14;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: i32 = 15;
pub const GHOSTTY_TERMINAL_DATA_WIDTH_PX: i32 = 16;
pub const GHOSTTY_TERMINAL_DATA_HEIGHT_PX: i32 = 17;
pub const GHOSTTY_TERMINAL_DATA_MODE: i32 = 37;
pub const GHOSTTY_TERMINAL_DATA_VT_GROUND: i32 = 38;
pub const GHOSTTY_TERMINAL_DATA_SELECTION: i32 = 31;

// `ghostty/vt/mouse/encoder.h`, `mouse/event.h`, `formatter.h`, `types.h`.
pub const GHOSTTY_MOUSE_ACTION_PRESS: i32 = 0;
pub const GHOSTTY_MOUSE_ACTION_RELEASE: i32 = 1;
pub const GHOSTTY_MOUSE_ACTION_MOTION: i32 = 2;

pub const GHOSTTY_MOUSE_BUTTON_UNKNOWN: i32 = 0;
pub const GHOSTTY_MOUSE_BUTTON_LEFT: i32 = 1;
pub const GHOSTTY_MOUSE_BUTTON_RIGHT: i32 = 2;
pub const GHOSTTY_MOUSE_BUTTON_MIDDLE: i32 = 3;
// Terminal wheel reporting maps wheel up/down to buttons 4/5.
pub const GHOSTTY_MOUSE_BUTTON_FOUR: i32 = 4;
pub const GHOSTTY_MOUSE_BUTTON_FIVE: i32 = 5;

pub const GHOSTTY_MOUSE_TRACKING_NONE: i32 = 0;
pub const GHOSTTY_MOUSE_FORMAT_X10: i32 = 0;
pub const GHOSTTY_MOUSE_FORMAT_SGR: i32 = 2;
pub const GHOSTTY_MOUSE_ENCODER_OPT_EVENT: i32 = 1;
pub const GHOSTTY_MOUSE_ENCODER_OPT_FORMAT: i32 = 2;
pub const GHOSTTY_MOUSE_ENCODER_OPT_SIZE: i32 = 3;
pub const GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED: i32 = 4;
pub const GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL: i32 = 5;

pub const GHOSTTY_FORMATTER_OPT_EMIT: i32 = 1;
pub const GHOSTTY_FORMATTER_OPT_UNWRAP: i32 = 2;
pub const GHOSTTY_FORMATTER_OPT_TRIM: i32 = 3;
pub const GHOSTTY_FORMATTER_OPT_SELECTION: i32 = 4;
pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: i32 = 0;

// `ghostty/vt/render.h`.
pub const GHOSTTY_RENDER_STATE_DIRTY_FALSE: i32 = 0;
pub const GHOSTTY_RENDER_STATE_DIRTY_PARTIAL: i32 = 1;
pub const GHOSTTY_RENDER_STATE_DIRTY_FULL: i32 = 2;

pub const GHOSTTY_RENDER_STATE_DATA_COLS: i32 = 1;
pub const GHOSTTY_RENDER_STATE_DATA_ROWS: i32 = 2;
pub const GHOSTTY_RENDER_STATE_DATA_DIRTY: i32 = 3;
pub const GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR: i32 = 4;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_BACKGROUND: i32 = 5;
pub const GHOSTTY_RENDER_STATE_DATA_COLOR_FOREGROUND: i32 = 6;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE: i32 = 10;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE: i32 = 11;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE: i32 = 14;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X: i32 = 15;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y: i32 = 16;
pub const GHOSTTY_RENDER_STATE_DATA_CURSOR: i32 = 18;
pub const GHOSTTY_RENDER_STATE_DATA_COLORS: i32 = 19;

pub const GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY: i32 = 1;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_CELLS: i32 = 3;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION: i32 = 4;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_VIEWPORT_Y: i32 = 6;
pub const GHOSTTY_RENDER_STATE_ROW_DATA_ID: i32 = 7;

pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE: i32 = 2;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN: i32 = 3;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF: i32 = 4;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR: i32 = 5;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR: i32 = 6;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED: i32 = 7;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING: i32 = 8;
pub const GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8: i32 = 9;

// `ghostty/vt/types.h`, `ghostty/vt/style.h`, `ghostty/vt/color.h`.
pub const GHOSTTY_CURSOR_STYLE_BAR: i32 = 0;
pub const GHOSTTY_CURSOR_STYLE_BLOCK: i32 = 1;
pub const GHOSTTY_CURSOR_STYLE_UNDERLINE: i32 = 2;
pub const GHOSTTY_CURSOR_STYLE_BLOCK_HOLLOW: i32 = 3;

pub const GHOSTTY_STYLE_FLAG_BOLD: u16 = 1 << 0;
pub const GHOSTTY_STYLE_FLAG_ITALIC: u16 = 1 << 1;
pub const GHOSTTY_STYLE_FLAG_FAINT: u16 = 1 << 2;
pub const GHOSTTY_STYLE_FLAG_BLINK: u16 = 1 << 3;
pub const GHOSTTY_STYLE_FLAG_INVERSE: u16 = 1 << 4;
pub const GHOSTTY_STYLE_FLAG_INVISIBLE: u16 = 1 << 5;
pub const GHOSTTY_STYLE_FLAG_STRIKETHROUGH: u16 = 1 << 6;
pub const GHOSTTY_STYLE_FLAG_OVERLINE: u16 = 1 << 7;
pub const GHOSTTY_STYLE_UNDERLINE_SHIFT: u16 = 8;
pub const GHOSTTY_STYLE_UNDERLINE_MASK: u16 = 0x7 << GHOSTTY_STYLE_UNDERLINE_SHIFT;

pub const GHOSTTY_STYLE_COLOR_NONE: u8 = 0;
pub const GHOSTTY_STYLE_COLOR_PALETTE: u8 = 1;
pub const GHOSTTY_STYLE_COLOR_RGB: u8 = 2;

// `ghostty/vt/key/encoder.h`, `ghostty/vt/key/event.h`.
pub const GHOSTTY_KEY_ENCODER_OPT_CURSOR_KEY_APPLICATION: i32 = 1;
pub const GHOSTTY_KEY_ENCODER_OPT_KEYPAD_KEY_APPLICATION: i32 = 2;
pub const GHOSTTY_KEY_ENCODER_OPT_ALT_ESC_PREFIX: i32 = 4;
pub const GHOSTTY_KEY_ENCODER_OPT_KITTY_FLAGS: i32 = 6;
pub const GHOSTTY_KEY_ENCODER_OPT_BACKARROW_KEY_MODE: i32 = 8;

/// DEC private mode 2004, packed exactly as `ghostty_mode_new(2004, false)`.
pub const GHOSTTY_MODE_BRACKETED_PASTE: u16 = 2004;

pub const GHOSTTY_KEY_ACTION_RELEASE: i32 = 0;
pub const GHOSTTY_KEY_ACTION_PRESS: i32 = 1;
pub const GHOSTTY_KEY_ACTION_REPEAT: i32 = 2;
pub const GHOSTTY_KEY_UNIDENTIFIED: i32 = 0;
pub const GHOSTTY_KEY_BACKQUOTE: i32 = 1;
pub const GHOSTTY_KEY_BACKSLASH: i32 = 2;
pub const GHOSTTY_KEY_BRACKET_LEFT: i32 = 3;
pub const GHOSTTY_KEY_BRACKET_RIGHT: i32 = 4;
pub const GHOSTTY_KEY_COMMA: i32 = 5;
pub const GHOSTTY_KEY_DIGIT_0: i32 = 6;
pub const GHOSTTY_KEY_DIGIT_1: i32 = 7;
pub const GHOSTTY_KEY_DIGIT_2: i32 = 8;
pub const GHOSTTY_KEY_DIGIT_3: i32 = 9;
pub const GHOSTTY_KEY_DIGIT_4: i32 = 10;
pub const GHOSTTY_KEY_DIGIT_5: i32 = 11;
pub const GHOSTTY_KEY_DIGIT_6: i32 = 12;
pub const GHOSTTY_KEY_DIGIT_7: i32 = 13;
pub const GHOSTTY_KEY_DIGIT_8: i32 = 14;
pub const GHOSTTY_KEY_DIGIT_9: i32 = 15;
pub const GHOSTTY_KEY_EQUAL: i32 = 16;
pub const GHOSTTY_KEY_A: i32 = 20;
pub const GHOSTTY_KEY_B: i32 = 21;
pub const GHOSTTY_KEY_C: i32 = 22;
pub const GHOSTTY_KEY_D: i32 = 23;
pub const GHOSTTY_KEY_E: i32 = 24;
pub const GHOSTTY_KEY_F: i32 = 25;
pub const GHOSTTY_KEY_G: i32 = 26;
pub const GHOSTTY_KEY_H: i32 = 27;
pub const GHOSTTY_KEY_I: i32 = 28;
pub const GHOSTTY_KEY_J: i32 = 29;
pub const GHOSTTY_KEY_K: i32 = 30;
pub const GHOSTTY_KEY_L: i32 = 31;
pub const GHOSTTY_KEY_M: i32 = 32;
pub const GHOSTTY_KEY_N: i32 = 33;
pub const GHOSTTY_KEY_O: i32 = 34;
pub const GHOSTTY_KEY_P: i32 = 35;
pub const GHOSTTY_KEY_Q: i32 = 36;
pub const GHOSTTY_KEY_R: i32 = 37;
pub const GHOSTTY_KEY_S: i32 = 38;
pub const GHOSTTY_KEY_T: i32 = 39;
pub const GHOSTTY_KEY_U: i32 = 40;
pub const GHOSTTY_KEY_V: i32 = 41;
pub const GHOSTTY_KEY_W: i32 = 42;
pub const GHOSTTY_KEY_X: i32 = 43;
pub const GHOSTTY_KEY_Y: i32 = 44;
pub const GHOSTTY_KEY_Z: i32 = 45;
pub const GHOSTTY_KEY_MINUS: i32 = 46;
pub const GHOSTTY_KEY_PERIOD: i32 = 47;
pub const GHOSTTY_KEY_QUOTE: i32 = 48;
pub const GHOSTTY_KEY_SEMICOLON: i32 = 49;
pub const GHOSTTY_KEY_SLASH: i32 = 50;
pub const GHOSTTY_KEY_BACKSPACE: i32 = 53;
pub const GHOSTTY_KEY_ENTER: i32 = 58;
pub const GHOSTTY_KEY_SPACE: i32 = 63;
pub const GHOSTTY_KEY_TAB: i32 = 64;
pub const GHOSTTY_KEY_DELETE: i32 = 68;
pub const GHOSTTY_KEY_END: i32 = 69;
pub const GHOSTTY_KEY_HOME: i32 = 71;
pub const GHOSTTY_KEY_INSERT: i32 = 72;
pub const GHOSTTY_KEY_PAGE_DOWN: i32 = 73;
pub const GHOSTTY_KEY_PAGE_UP: i32 = 74;
pub const GHOSTTY_KEY_ARROW_DOWN: i32 = 75;
pub const GHOSTTY_KEY_ARROW_LEFT: i32 = 76;
pub const GHOSTTY_KEY_ARROW_RIGHT: i32 = 77;
pub const GHOSTTY_KEY_ARROW_UP: i32 = 78;
pub const GHOSTTY_KEY_ESCAPE: i32 = 120;
pub const GHOSTTY_KEY_F1: i32 = 121;
pub const GHOSTTY_KEY_F2: i32 = 122;
pub const GHOSTTY_KEY_F3: i32 = 123;
pub const GHOSTTY_KEY_F4: i32 = 124;
pub const GHOSTTY_KEY_F5: i32 = 125;
pub const GHOSTTY_KEY_F6: i32 = 126;
pub const GHOSTTY_KEY_F7: i32 = 127;
pub const GHOSTTY_KEY_F8: i32 = 128;
pub const GHOSTTY_KEY_F9: i32 = 129;
pub const GHOSTTY_KEY_F10: i32 = 130;
pub const GHOSTTY_KEY_F11: i32 = 131;
pub const GHOSTTY_KEY_F12: i32 = 132;
pub const GHOSTTY_MODS_SHIFT: u16 = 1 << 0;
pub const GHOSTTY_MODS_CTRL: u16 = 1 << 1;
pub const GHOSTTY_MODS_ALT: u16 = 1 << 2;
pub const GHOSTTY_MODS_SUPER: u16 = 1 << 3;

// `ghostty/vt/paste.h`, `ghostty/vt/terminal.h`.
pub const GHOSTTY_CLIPBOARD_LOCATION_STANDARD: i32 = 0;
pub const GHOSTTY_PASTE_SOURCE_CLIPBOARD: i32 = 0;
pub const GHOSTTY_PASTE_SOURCE_TEXT: i32 = 1;

// `ghostty/vt/point.h` and `ghostty/vt/selection.h`: fixture selection.
pub const GHOSTTY_POINT_TAG_ACTIVE: i32 = 0;
pub const GHOSTTY_POINT_TAG_VIEWPORT: i32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyStyleColorValue {
    pub palette: u8,
    pub rgb: GhosttyColorRgb,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyleColor {
    pub tag: u8,
    pub value: GhosttyStyleColorValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyStyle {
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub flags: u16,
    pub reserved: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyBuffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyCodepointBuffer {
    pub ptr: *mut u32,
    pub cap: usize,
    pub len: usize,
}

pub type GhosttyWriterFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool;
pub type GhosttyMimeReaderFn =
    unsafe extern "C" fn(*mut c_void, GhosttyString, GhosttyWriter) -> bool;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyWriter {
    pub write: Option<GhosttyWriterFn>,
    pub userdata: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyMimeReader {
    pub read: Option<GhosttyMimeReaderFn>,
    pub userdata: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPaste {
    /// Set with `ghostty_sized_init` before passing to `ghostty_terminal_paste`.
    pub size: usize,
    pub location: i32,
    pub source: i32,
    pub mimes: *const GhosttyString,
    pub mimes_len: usize,
    pub reader: GhosttyMimeReader,
    pub allow_unsafe: bool,
}

/// `GhosttyTerminalModeConfig` (`ghostty/vt/terminal.h`), frozen 4 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyTerminalModeConfig {
    pub mode: u16,
    pub value: bool,
}

/// Tagged value for `GhosttyTerminalScrollViewport`. The C union is 16 bytes
/// (`uint64_t[2]` padding), and the tag is an int-sized enum.
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    pub delta: isize,
    pub row: usize,
    pub delta_prompt: isize,
    pub padding: [u64; 2],
}

/// By-value argument to `ghostty_terminal_scroll_viewport`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: i32,
    pub value: GhosttyTerminalScrollViewportValue,
}

impl GhosttyTerminalScrollViewport {
    #[must_use]
    pub fn top() -> Self {
        Self {
            tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP,
            value: GhosttyTerminalScrollViewportValue { padding: [0; 2] },
        }
    }

    #[must_use]
    pub fn bottom() -> Self {
        Self {
            tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM,
            value: GhosttyTerminalScrollViewportValue { padding: [0; 2] },
        }
    }

    /// Negative rows scroll up into history; positive rows scroll down.
    #[must_use]
    pub fn delta(rows: isize) -> Self {
        Self {
            tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
            value: GhosttyTerminalScrollViewportValue { delta: rows },
        }
    }

    /// Absolute row from the top of the scrollable area.
    #[must_use]
    pub fn row(row: usize) -> Self {
        Self {
            tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW,
            value: GhosttyTerminalScrollViewportValue { row },
        }
    }
}

/// Scrollbar state read with `GHOSTTY_TERMINAL_DATA_SCROLLBAR`. All fields are
/// row counts in the same coordinate space as `scroll_viewport(ROW)`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyTerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    pub padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    pub tag: i32,
    pub value: GhosttyPointValue,
}

impl GhosttyPoint {
    #[must_use]
    pub fn viewport(x: u16, y: u32) -> Self {
        Self {
            tag: GHOSTTY_POINT_TAG_VIEWPORT,
            value: GhosttyPointValue {
                coordinate: GhosttyPointCoordinate { x, y },
            },
        }
    }
}

/// Frozen 16-byte struct (`ghostty/vt/grid_ref.h`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GhosttyGridRef {
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

/// Frozen 40-byte struct on 64-bit targets (`ghostty/vt/selection.h`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttySelection {
    pub start: GhosttyGridRef,
    pub end: GhosttyGridRef,
    pub rectangle: bool,
}

/// `GhosttyMousePosition` from `ghostty/vt/mouse/event.h`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GhosttyMousePosition {
    pub x: f32,
    pub y: f32,
}

/// Sized struct from `ghostty/vt/mouse/encoder.h`. Geometry is the surface
/// context used to convert pixel positions to terminal cell coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GhosttyMouseEncoderSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

impl GhosttyMouseEncoderSize {
    #[must_use]
    pub fn sized() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            screen_width: 0,
            screen_height: 0,
            cell_width: 1,
            cell_height: 1,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateRowId {
    pub bits: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateRowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

impl GhosttyRenderStateCursor {
    /// Equivalent to `GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor)`.
    #[must_use]
    pub fn sized() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            viewport_has_value: false,
            viewport_x: 0,
            viewport_y: 0,
            wide_tail: false,
            visible: false,
            blinking: false,
            password_input: false,
            visual_style: 0,
        }
    }
}

impl GhosttyRenderStateRowSelection {
    #[must_use]
    pub fn sized() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            start_x: 0,
            end_x: 0,
        }
    }
}

impl GhosttyRenderStateColors {
    #[must_use]
    pub fn sized() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            background: GhosttyColorRgb::default(),
            foreground: GhosttyColorRgb::default(),
            cursor: GhosttyColorRgb::default(),
            cursor_has_value: false,
            palette: [GhosttyColorRgb::default(); 256],
        }
    }
}

impl GhosttyPaste {
    #[must_use]
    pub fn sized() -> Self {
        Self {
            size: core::mem::size_of::<Self>(),
            location: GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
            source: GHOSTTY_PASTE_SOURCE_TEXT,
            mimes: core::ptr::null(),
            mimes_len: 0,
            reader: GhosttyMimeReader {
                read: None,
                userdata: core::ptr::null_mut(),
            },
            allow_unsafe: false,
        }
    }
}

unsafe extern "C" {
    // `ghostty/vt/types.h` and `ghostty/vt/build_info.h`.
    pub fn ghostty_sized_init(ptr: *mut c_void, size: usize);
    pub fn ghostty_version() -> *const c_char;
    pub fn ghostty_style_default(style: *mut GhosttyStyle);

    // `ghostty/vt/color.h`. The palette generator is part of the stable VT
    // library; it keeps the base-16 colors and fills indices 16..256.
    pub fn ghostty_color_palette_default(out: *mut GhosttyColorRgb);
    pub fn ghostty_color_palette_generate(
        base: *const GhosttyColorRgb,
        skip: *const c_void,
        bg: *const GhosttyColorRgb,
        fg: *const GhosttyColorRgb,
        harmonious: bool,
        out: *mut GhosttyColorRgb,
    );

    // `ghostty/vt/terminal.h`.
    pub fn ghostty_terminal_new(
        allocator: *const c_void,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_reset(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: i32,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        data: i32,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    /// Scroll the viewport. `behavior` is passed by value (24 bytes).
    pub fn ghostty_terminal_scroll_viewport(
        terminal: GhosttyTerminal,
        behavior: GhosttyTerminalScrollViewport,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_select_all(
        terminal: GhosttyTerminal,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;

    // `ghostty/vt/render.h`.
    pub fn ghostty_render_state_new(
        allocator: *const c_void,
        state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_begin_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_end_update(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        data: i32,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const c_void,
        out_iterator: *mut GhosttyRenderStateRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iterator: GhosttyRenderStateRowIterator);
    pub fn ghostty_render_state_row_iterator_next(iterator: GhosttyRenderStateRowIterator) -> bool;
    pub fn ghostty_render_state_row_iterator_next_dirty(
        iterator: GhosttyRenderStateRowIterator,
        out_y: *mut u16,
    ) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: GhosttyRenderStateRowIterator,
        data: i32,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_cells_new(
        allocator: *const c_void,
        out_cells: *mut GhosttyRenderStateRowCells,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRenderStateRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_select(
        cells: GhosttyRenderStateRowCells,
        x: u16,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRenderStateRowCells,
        data: i32,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRenderStateRowCells);

    // `ghostty/vt/key/encoder.h` and `ghostty/vt/key/event.h`.
    pub fn ghostty_key_encoder_new(
        allocator: *const c_void,
        encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    pub fn ghostty_key_encoder_set(
        encoder: GhosttyKeyEncoder,
        option: i32,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_key_encoder_set_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_new(
        allocator: *const c_void,
        event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: i32) -> GhosttyResult;
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: i32) -> GhosttyResult;
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: u16) -> GhosttyResult;
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: GhosttyString)
        -> GhosttyResult;
    pub fn ghostty_key_event_set_unshifted_codepoint(
        event: GhosttyKeyEvent,
        codepoint: u32,
    ) -> GhosttyResult;

    // `ghostty/vt/paste.h`.
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_pasted: *mut bool,
    ) -> GhosttyResult;
    pub fn ghostty_paste_is_safe(data: *const c_char, len: usize) -> bool;
    pub fn ghostty_paste_encode(
        data: *mut c_char,
        data_len: usize,
        bracketed: bool,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;

    // `ghostty/vt/mouse/encoder.h` and `mouse/event.h`.
    pub fn ghostty_mouse_encoder_new(
        allocator: *const c_void,
        encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);
    pub fn ghostty_mouse_encoder_set(
        encoder: GhosttyMouseEncoder,
        option: i32,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_encoder_set_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_encoder_reset(encoder: GhosttyMouseEncoder) -> GhosttyResult;
    pub fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_event_new(
        allocator: *const c_void,
        event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;
    pub fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
    pub fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: i32) -> GhosttyResult;
    pub fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: i32) -> GhosttyResult;
    pub fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent) -> GhosttyResult;
    pub fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: u16) -> GhosttyResult;
    pub fn ghostty_mouse_event_set_position(
        event: GhosttyMouseEvent,
        position: GhosttyMousePosition,
    ) -> GhosttyResult;

    // `ghostty/vt/formatter.h`: selection text for clipboard copy.
    pub fn ghostty_formatter_terminal_new(
        allocator: *const c_void,
        formatter: *mut GhosttyFormatter,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_set(
        formatter: GhosttyFormatter,
        option: i32,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_format_alloc(
        formatter: GhosttyFormatter,
        allocator: *const c_void,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_formatter_free(formatter: GhosttyFormatter);
    pub fn ghostty_free(ptr: *mut c_void);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    /// These values were measured with `zig cc` against the pinned headers.
    /// A mismatch means the Rust declarations no longer describe the library.
    #[test]
    fn abi_struct_sizes_match_pinned_headers() {
        assert_eq!(size_of::<GhosttyColorRgb>(), 3);
        assert_eq!(size_of::<GhosttyStyleColor>(), 4);
        assert_eq!(size_of::<GhosttyStyle>(), 16);
        assert_eq!(size_of::<GhosttyRenderStateCursor>(), 24);
        assert_eq!(size_of::<GhosttyRenderStateColors>(), 792);
        assert_eq!(size_of::<GhosttyRenderStateRowSelection>(), 16);
        assert_eq!(size_of::<GhosttyRenderStateRowId>(), 16);
        assert_eq!(size_of::<GhosttyPaste>(), 56);
        assert_eq!(size_of::<GhosttyTerminalModeConfig>(), 4);
        assert_eq!(size_of::<GhosttyPoint>(), 24);
        assert_eq!(size_of::<GhosttyGridRef>(), 16);
        assert_eq!(size_of::<GhosttySelection>(), 40);
        assert_eq!(size_of::<GhosttyString>(), 16);
        assert_eq!(size_of::<GhosttyBuffer>(), 24);
        assert_eq!(size_of::<GhosttyCodepointBuffer>(), 24);
        assert_eq!(size_of::<GhosttyWriter>(), 16);
        assert_eq!(size_of::<GhosttyMousePosition>(), 8);
        assert_eq!(size_of::<GhosttyMouseEncoderSize>(), 40);
        assert_eq!(size_of::<GhosttyTerminalScrollViewport>(), 24);
        assert_eq!(size_of::<GhosttyTerminalScrollbar>(), 24);
    }
}
