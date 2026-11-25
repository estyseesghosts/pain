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

/// Unicode codepoint of the physical key with no modifiers. Ghostty uses this
/// to tell shifted/control characters apart (CSI-u, `modifyOtherKeys`, caps
/// lock). GPUI's `Keystroke::key` already names the physical key in lowercase,
/// so this is a pure lookup rather than a second keyboard-layout model.
#[must_use]
pub fn unshifted_codepoint(name: &str) -> u32 {
    match name {
        // The only printable key whose GPUI name is not a single character.
        "space" => u32::from(' '),
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => u32::from(ch.to_ascii_lowercase()),
                _ => 0,
            }
        }
    }
}

/// Everything Ghostty's key encoder needs for one keystroke, derived from a
/// platform-neutral keystroke. `None` means the platform `InputHandler` owns
/// the text (AltGr/dead keys) and the view must not encode the physical key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedKey {
    pub key: i32,
    pub mods: Modifiers,
    /// Committed UTF-8 text, attached even for named printable keys.
    pub text: Option<String>,
    pub unshifted_codepoint: u32,
    pub repeat: bool,
}

/// Decide whether a keystroke belongs to Ghostty's encoder or to the platform
/// text-input path. `None` means the view must let the event propagate so the
/// native handler delivers the committed text.
///
/// The ownership split matches Zed's pinned terminal:
///
/// - AltGr/dead keys (`prefer_character_input`) are composed text on Windows;
///   GPUI delivers the result through `InputHandler::replace_text_in_range`.
/// - Plain printable keys (shift allowed, but no Ctrl/Alt/Super) are delivered
///   as text by every platform: WM_CHAR on Windows, and X11/Wayland's
///   key_char forwarding when propagation continues on Linux. Encoding them
///   here as well would double every character on Windows, and the space bar
///   (whose GPUI name is `"space"`) would be dropped entirely on Linux.
/// - Everything else (control keys, arrows, Ctrl/Alt combinations) has no
///   native text equivalent and must be encoded by Ghostty.
#[must_use]
pub fn key_for_keystroke(
    name: &str,
    key_char: Option<&str>,
    mods: Modifiers,
    prefer_character_input: bool,
    is_held: bool,
) -> Option<EncodedKey> {
    if key_char.is_some() {
        if prefer_character_input {
            return None;
        }
        // Only shift may accompany text that the platform will deliver.
        if !mods.control && !mods.alt && !mods.super_key {
            return None;
        }
    }
    // Single-character names are a fallback for platforms that do not supply
    // key_char. Named keys with no key_char (Enter, arrows, ...) carry no text
    // and must not invent it from their name.
    let text = key_char
        .map(str::to_owned)
        .or_else(|| (name.chars().count() == 1).then(|| name.to_owned()));
    Some(EncodedKey {
        key: key_for_name(name),
        mods,
        text,
        unshifted_codepoint: unshifted_codepoint(name),
        repeat: is_held,
    })
}

/// Keys bound to application commands rather than the PTY. GPUI dispatches
/// these bindings before the terminal's `on_key_down` listener; the view must
/// not also encode them, or a Ctrl+Shift+C in a TUI becomes a stray escape
/// sequence after the clipboard copy. Zed's pinned keymaps use the same
/// bindings for the terminal: Ctrl+Shift+C/V and Shift+Insert.
#[must_use]
pub fn is_app_shortcut(name: &str, mods: Modifiers) -> bool {
    let exact = |expected: &str, ctrl: bool, shift: bool| {
        name == expected
            && mods.control == ctrl
            && mods.shift == shift
            && !mods.alt
            && !mods.super_key
    };
    exact("c", true, true) // Ctrl+Shift+C: copy
        || exact("v", true, true) // Ctrl+Shift+V: paste
        || exact("a", true, true) // Ctrl+Shift+A: select all
        || ((name == "," || name == "comma")
            && mods.control
            && !mods.shift
            && !mods.alt
            && !mods.super_key) // Ctrl+,: preferences
        || (name == "insert" && mods.shift && !mods.control && !mods.alt && !mods.super_key)
}

/// True when the user is asking for local selection instead of terminal mouse
/// reporting. Shift is the conventional override (xterm/kitty); Super is kept
/// as a second explicit escape because it is already the platform menu key.
#[must_use]
pub fn mouse_escape(mods: Modifiers) -> bool {
    mods.shift || mods.super_key
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

    /// Bit used to track which buttons are currently held. Ghostty needs this
    /// as `any_button_pressed` so drags that leave the surface are still
    /// reported in motion-tracking modes.
    #[must_use]
    pub fn bit(self) -> u8 {
        match self {
            Self::Left => 1,
            Self::Right => 2,
            Self::Middle => 4,
            Self::None => 0,
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
    fn plain_printable_keys_delegate_to_the_text_handler() {
        // GPUI names the space bar "space" on both platforms. Encoding it
        // produced no bytes (Ghostty needs UTF-8 for printable keys) and WM_CHAR
        // also delivered the text, so spaces were at best doubled and on Linux
        // lost entirely. Both paths must agree to delegate.
        for (name, text) in [("space", " "), ("a", "a"), ("2", "2"), (".", ".")] {
            assert!(
                key_for_keystroke(name, Some(text), Modifiers::default(), false, false).is_none(),
                "{name} must be delivered as text"
            );
            // Shift-only printable keys are also plain text on every platform.
            assert!(
                key_for_keystroke(
                    name,
                    Some(text),
                    Modifiers::new(true, false, false, false),
                    false,
                    false,
                )
                .is_none(),
                "shift+{name} must be delivered as text"
            );
        }
    }

    #[test]
    fn altgr_and_dead_keys_delegate_to_the_text_handler() {
        // Windows sets prefer_character_input when the composed character
        // differs from the unmodified key (AltGr) or a dead key is active.
        assert!(key_for_keystroke(
            "2",
            Some("²"),
            Modifiers::new(false, true, true, false),
            true,
            false,
        )
        .is_none());
    }

    #[test]
    fn named_control_keys_have_no_text_but_keep_their_key_code() {
        for name in ["enter", "backspace", "tab", "escape", "up"] {
            let encoded =
                key_for_keystroke(name, None, Modifiers::default(), false, false).expect("encoded");
            assert_eq!(encoded.text, None, "{name} must not invent text");
            assert_ne!(
                encoded.key,
                sys::GHOSTTY_KEY_UNIDENTIFIED,
                "{name} needs a Ghostty key code"
            );
            assert_eq!(encoded.unshifted_codepoint, 0, "{name} is not printable");
        }
    }

    #[test]
    fn modified_keys_are_encoded_with_correct_unshifted_codepoint() {
        // Ctrl+Shift+A commits no printable character, so Ghostty encodes it.
        let encoded = key_for_keystroke(
            "a",
            None,
            Modifiers::new(true, true, false, false),
            false,
            true,
        )
        .expect("encoded");
        assert!(encoded.repeat);
        // The unshifted physical key is 'a', not the committed character.
        assert_eq!(encoded.unshifted_codepoint, u32::from('a'));
        assert_eq!(encoded.text.as_deref(), Some("a"));
        assert_eq!(
            encoded.mods.ghostty_bits(),
            sys::GHOSTTY_MODS_SHIFT | sys::GHOSTTY_MODS_CTRL
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

    #[test]
    fn app_shortcuts_are_not_key_sequences_for_the_pty() {
        assert!(is_app_shortcut(
            "c",
            Modifiers::new(true, true, false, false)
        ));
        assert!(is_app_shortcut(
            "v",
            Modifiers::new(true, true, false, false)
        ));
        assert!(is_app_shortcut(
            "a",
            Modifiers::new(true, true, false, false)
        ));
        assert!(is_app_shortcut(
            "insert",
            Modifiers::new(true, false, false, false)
        ));
        assert!(is_app_shortcut(
            ",",
            Modifiers::new(false, true, false, false)
        ));
        // Plain Ctrl+C and unmodified keys still belong to the PTY.
        assert!(!is_app_shortcut(
            "c",
            Modifiers::new(false, true, false, false)
        ));
        assert!(!is_app_shortcut("c", Modifiers::default()));
        assert!(!is_app_shortcut("insert", Modifiers::default()));
    }

    #[test]
    fn shift_or_super_requests_local_mouse_selection() {
        assert!(!mouse_escape(Modifiers::default()));
        assert!(mouse_escape(Modifiers::new(true, false, false, false)));
        assert!(mouse_escape(Modifiers::new(false, false, false, true)));
        assert!(!mouse_escape(Modifiers::new(false, true, false, false)));
    }

    #[test]
    fn held_mouse_buttons_have_distinct_bits() {
        let bits = [
            MouseButton::Left.bit(),
            MouseButton::Right.bit(),
            MouseButton::Middle.bit(),
        ];
        assert_eq!(bits, [1, 2, 4]);
        assert_eq!(MouseButton::None.bit(), 0);
    }
}
