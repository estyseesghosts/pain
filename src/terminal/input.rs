//! Pure, headless-testable mapping from platform-neutral input descriptions
//! to Ghostty key/mouse values. GPUI's `Keystroke` is converted to `KeyInput`
//! by the view layer; no ANSI escape sequence is hand-written here — Ghostty's
//! encoders own that policy (AGENTS.md).

use crate::terminal::ghostty::ghostty_vt_sys as sys;

/// Platform-neutral modifier set. Mirrors GPUI's `Modifiers` without importing
/// GPUI so these rules can be tested in headless M0/M3/M4 builds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl Modifiers {
    #[must_use]
    pub fn new(shift: bool, control: bool, alt: bool, super_key: bool) -> Self {
        Self {
            shift,
            control,
            alt,
            super_key,
        }
    }

    /// Pack into Ghostty's `GhosttyMods` bitmask.
    #[must_use]
    pub fn ghostty_bits(self) -> u16 {
        let mut bits = 0u16;
        if self.shift {
            bits |= sys::GHOSTTY_MODS_SHIFT;
        }
        if self.control {
            bits |= sys::GHOSTTY_MODS_CTRL;
        }
        if self.alt {
            bits |= sys::GHOSTTY_MODS_ALT;
        }
        if self.super_key {
            bits |= sys::GHOSTTY_MODS_SUPER;
        }
        bits
    }
}

/// Whitelisted application commands driven by Ctrl+key. Font shortcuts are
/// app-level commands, never terminal input, so they are resolved before the
/// key reaches Ghostty's encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontCommand {
    Increase,
    Decrease,
    Reset,
}

/// Resolve a font command from a keystroke. GPUI reports the same physical key
/// with different names on Windows and Linux (`-`, `minus`, `_`), and a
/// shifted or IME-produced event can leave the useful spelling in `key_char`
/// instead of `key`. Keeping this mapping pure lets every platform spelling be
/// covered by a headless unit test.
#[must_use]
pub fn font_command(name: &str, key_char: Option<&str>, control: bool) -> Option<FontCommand> {
    if !control {
        return None;
    }
    match name {
        "=" | "+" | "equal" | "plus" => Some(FontCommand::Increase),
        "-" | "_" | "minus" | "underscore" | "subtract" => Some(FontCommand::Decrease),
        "0" | "zero" => Some(FontCommand::Reset),
        _ => match key_char {
            Some("=") | Some("+") => Some(FontCommand::Increase),
            Some("-") | Some("_") => Some(FontCommand::Decrease),
            Some("0") => Some(FontCommand::Reset),
            _ => None,
        },
    }
}

/// One key press ready to be fed to `GhosttyKeyEncoder`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    /// Ghostty key code (`GHOSTTY_KEY_*`).
    pub key: i32,
    pub mods: Modifiers,
    /// Committed UTF-8 text for printable keys / IME. `None` for control keys.
    pub text: Option<String>,
}

impl KeyInput {
    #[must_use]
    pub fn named(key: i32, mods: Modifiers) -> Self {
        Self {
            key,
            mods,
            text: None,
        }
    }

    #[must_use]
    pub fn character(ch: char, mods: Modifiers) -> Self {
        Self {
            key: key_for_ascii(ch).unwrap_or(sys::GHOSTTY_KEY_UNIDENTIFIED),
            mods,
            text: Some(ch.to_string()),
        }
    }
}

/// Map a GPUI key name (or a single printable character) to a Ghostty key code.
/// Returns `GHOSTTY_KEY_UNIDENTIFIED` for keys with no Ghostty equivalent.
#[must_use]
pub fn key_for_name(name: &str) -> i32 {
    if let Some(ch) = single_char(name) {
        if let Some(key) = key_for_ascii(ch) {
            return key;
        }
    }
    match name {
        "backspace" => sys::GHOSTTY_KEY_BACKSPACE,
        "tab" => sys::GHOSTTY_KEY_TAB,
        "enter" => sys::GHOSTTY_KEY_ENTER,
        "space" => sys::GHOSTTY_KEY_SPACE,
        "escape" | "esc" => sys::GHOSTTY_KEY_ESCAPE,
        "delete" => sys::GHOSTTY_KEY_DELETE,
        "insert" => sys::GHOSTTY_KEY_INSERT,
        "home" => sys::GHOSTTY_KEY_HOME,
        "end" => sys::GHOSTTY_KEY_END,
        "pageup" => sys::GHOSTTY_KEY_PAGE_UP,
        "pagedown" => sys::GHOSTTY_KEY_PAGE_DOWN,
        "up" => sys::GHOSTTY_KEY_ARROW_UP,
        "down" => sys::GHOSTTY_KEY_ARROW_DOWN,
        "left" => sys::GHOSTTY_KEY_ARROW_LEFT,
        "right" => sys::GHOSTTY_KEY_ARROW_RIGHT,
        "f1" => sys::GHOSTTY_KEY_F1,
        "f2" => sys::GHOSTTY_KEY_F2,
        "f3" => sys::GHOSTTY_KEY_F3,
        "f4" => sys::GHOSTTY_KEY_F4,
        "f5" => sys::GHOSTTY_KEY_F5,
        "f6" => sys::GHOSTTY_KEY_F6,
        "f7" => sys::GHOSTTY_KEY_F7,
        "f8" => sys::GHOSTTY_KEY_F8,
        "f9" => sys::GHOSTTY_KEY_F9,
        "f10" => sys::GHOSTTY_KEY_F10,
        "f11" => sys::GHOSTTY_KEY_F11,
        "f12" => sys::GHOSTTY_KEY_F12,
        _ => sys::GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn single_char(name: &str) -> Option<char> {
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        Some(first)
    } else {
        None
    }
}

fn key_for_ascii(ch: char) -> Option<i32> {
    let key = match ch.to_ascii_lowercase() {
        '`' => sys::GHOSTTY_KEY_BACKQUOTE,
        '\\' => sys::GHOSTTY_KEY_BACKSLASH,
        '[' => sys::GHOSTTY_KEY_BRACKET_LEFT,
        ']' => sys::GHOSTTY_KEY_BRACKET_RIGHT,
        ',' => sys::GHOSTTY_KEY_COMMA,
        '0' => sys::GHOSTTY_KEY_DIGIT_0,
        '1' => sys::GHOSTTY_KEY_DIGIT_1,
        '2' => sys::GHOSTTY_KEY_DIGIT_2,
        '3' => sys::GHOSTTY_KEY_DIGIT_3,
        '4' => sys::GHOSTTY_KEY_DIGIT_4,
        '5' => sys::GHOSTTY_KEY_DIGIT_5,
        '6' => sys::GHOSTTY_KEY_DIGIT_6,
        '7' => sys::GHOSTTY_KEY_DIGIT_7,
        '8' => sys::GHOSTTY_KEY_DIGIT_8,
        '9' => sys::GHOSTTY_KEY_DIGIT_9,
        '=' => sys::GHOSTTY_KEY_EQUAL,
        'a' => sys::GHOSTTY_KEY_A,
        'b' => sys::GHOSTTY_KEY_B,
        'c' => sys::GHOSTTY_KEY_C,
        'd' => sys::GHOSTTY_KEY_D,
        'e' => sys::GHOSTTY_KEY_E,
        'f' => sys::GHOSTTY_KEY_F,
        'g' => sys::GHOSTTY_KEY_G,
        'h' => sys::GHOSTTY_KEY_H,
        'i' => sys::GHOSTTY_KEY_I,
        'j' => sys::GHOSTTY_KEY_J,
        'k' => sys::GHOSTTY_KEY_K,
        'l' => sys::GHOSTTY_KEY_L,
        'm' => sys::GHOSTTY_KEY_M,
        'n' => sys::GHOSTTY_KEY_N,
        'o' => sys::GHOSTTY_KEY_O,
        'p' => sys::GHOSTTY_KEY_P,
        'q' => sys::GHOSTTY_KEY_Q,
        'r' => sys::GHOSTTY_KEY_R,
        's' => sys::GHOSTTY_KEY_S,
        't' => sys::GHOSTTY_KEY_T,
        'u' => sys::GHOSTTY_KEY_U,
        'v' => sys::GHOSTTY_KEY_V,
        'w' => sys::GHOSTTY_KEY_W,
        'x' => sys::GHOSTTY_KEY_X,
        'y' => sys::GHOSTTY_KEY_Y,
        'z' => sys::GHOSTTY_KEY_Z,
        '-' => sys::GHOSTTY_KEY_MINUS,
        '.' => sys::GHOSTTY_KEY_PERIOD,
        '\'' => sys::GHOSTTY_KEY_QUOTE,
        ';' => sys::GHOSTTY_KEY_SEMICOLON,
        '/' => sys::GHOSTTY_KEY_SLASH,
        ' ' => sys::GHOSTTY_KEY_SPACE,
        _ => return None,
    };
    Some(key)
}

/// Mouse actions/buttons in Ghostty's numeric representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

impl MouseAction {
    #[must_use]
    pub fn ghostty(self) -> i32 {
        match self {
            Self::Press => sys::GHOSTTY_MOUSE_ACTION_PRESS,
            Self::Release => sys::GHOSTTY_MOUSE_ACTION_RELEASE,
            Self::Motion => sys::GHOSTTY_MOUSE_ACTION_MOTION,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    None,
}

impl MouseButton {
    #[must_use]
    pub fn ghostty(self) -> Option<i32> {
        match self {
            Self::Left => Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT),
            Self::Right => Some(sys::GHOSTTY_MOUSE_BUTTON_RIGHT),
            Self::Middle => Some(sys::GHOSTTY_MOUSE_BUTTON_MIDDLE),
            Self::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_commands_cover_every_platform_spelling() {
        // Main-row keys on Windows and Linux.
        assert_eq!(font_command("=", None, true), Some(FontCommand::Increase));
        assert_eq!(font_command("-", None, true), Some(FontCommand::Decrease));
        assert_eq!(font_command("0", None, true), Some(FontCommand::Reset));
        // Named and shifted spellings.
        assert_eq!(
            font_command("equal", None, true),
            Some(FontCommand::Increase)
        );
        assert_eq!(
            font_command("minus", None, true),
            Some(FontCommand::Decrease)
        );
        assert_eq!(font_command("_", None, true), Some(FontCommand::Decrease));
        assert_eq!(
            font_command("underscore", None, true),
            Some(FontCommand::Decrease)
        );
        assert_eq!(
            font_command("subtract", None, true),
            Some(FontCommand::Decrease)
        );
        // The committed character is the only signal on some IME paths.
        assert_eq!(
            font_command("unknown", Some("-"), true),
            Some(FontCommand::Decrease)
        );
        assert_eq!(
            font_command("unknown", Some("+"), true),
            Some(FontCommand::Increase)
        );
        // Without Ctrl, these characters belong to the shell.
        assert_eq!(font_command("-", None, false), None);
        assert_eq!(font_command("a", Some("a"), true), None);
    }

    #[test]
    fn modifiers_pack_ghostty_bits() {
        assert_eq!(Modifiers::default().ghostty_bits(), 0);
        assert_eq!(
            Modifiers::new(true, true, true, true).ghostty_bits(),
            sys::GHOSTTY_MODS_SHIFT
                | sys::GHOSTTY_MODS_CTRL
                | sys::GHOSTTY_MODS_ALT
                | sys::GHOSTTY_MODS_SUPER
        );
    }

    #[test]
    fn named_and_character_keys_map_to_ghostty() {
        assert_eq!(key_for_name("enter"), sys::GHOSTTY_KEY_ENTER);
        assert_eq!(key_for_name("backspace"), sys::GHOSTTY_KEY_BACKSPACE);
        assert_eq!(key_for_name("escape"), sys::GHOSTTY_KEY_ESCAPE);
        assert_eq!(key_for_name("up"), sys::GHOSTTY_KEY_ARROW_UP);
        assert_eq!(key_for_name("f12"), sys::GHOSTTY_KEY_F12);
        assert_eq!(key_for_name("A"), sys::GHOSTTY_KEY_A);
        assert_eq!(key_for_name("7"), sys::GHOSTTY_KEY_DIGIT_7);
        assert_eq!(key_for_name("space"), sys::GHOSTTY_KEY_SPACE);
        assert_eq!(key_for_name("unknown"), sys::GHOSTTY_KEY_UNIDENTIFIED);
        assert_eq!(
            KeyInput::character('c', Modifiers::new(false, true, false, false)),
            KeyInput {
                key: sys::GHOSTTY_KEY_C,
                mods: Modifiers::new(false, true, false, false),
                text: Some("c".into()),
            }
        );
    }

    #[test]
    fn mouse_actions_and_buttons_map_to_ghostty() {
        assert_eq!(
            MouseAction::Press.ghostty(),
            sys::GHOSTTY_MOUSE_ACTION_PRESS
        );
        assert_eq!(
            MouseAction::Motion.ghostty(),
            sys::GHOSTTY_MOUSE_ACTION_MOTION
        );
        assert_eq!(
            MouseButton::Left.ghostty(),
            Some(sys::GHOSTTY_MOUSE_BUTTON_LEFT)
        );
        assert_eq!(MouseButton::None.ghostty(), None);
    }
}
