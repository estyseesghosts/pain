//! Pain-owned keyboard shortcut policy.
//!
//! Terminal translation rules live in `pain-terminal-core`; this module holds
//! only the host's choice of which Ctrl/Shift keys are application commands.
//! A reusable terminal surface must not decide that `Ctrl+,` opens Pain's
//! preferences, so this allowlist stays in the application.

use pain_terminal_core::input::Modifiers;

/// Whitelisted application commands driven by Ctrl/Shift+key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    IncreaseFont,
    DecreaseFont,
    ResetFont,
    CopySelection,
    PasteClipboard,
    SelectAll,
    OpenPreferences,
}

/// Font size commands resolved from the many platform spellings of `+`, `-`
/// and `0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontCommand {
    Increase,
    Decrease,
    Reset,
}

/// Resolve a font command from a keystroke. GPUI reports the same physical key
/// with different names on Windows and Linux (`-`, `minus`, `_`), and a shifted
/// or IME-produced event can leave the useful spelling in `key_char` instead of
/// `key`.
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

/// Resolve a keystroke to one of Pain's application commands. Shared by the
/// raw interceptor (which must run before native text handling on Windows) and
/// by the terminal view's ownership check so both agree on a key.
#[must_use]
pub fn app_command(name: &str, key_char: Option<&str>, mods: Modifiers) -> Option<AppCommand> {
    if name.eq_ignore_ascii_case("insert")
        && mods.shift
        && !mods.control
        && !mods.alt
        && !mods.super_key
    {
        return Some(AppCommand::PasteClipboard); // Shift+Insert
    }
    if !mods.control || mods.alt || mods.super_key {
        return None; // AltGr is composed text, not a Ctrl shortcut.
    }
    if mods.shift {
        return match name.to_ascii_lowercase().as_str() {
            "c" => Some(AppCommand::CopySelection),
            "v" => Some(AppCommand::PasteClipboard),
            "a" => Some(AppCommand::SelectAll),
            _ => None,
        };
    }
    if name == "," || name.eq_ignore_ascii_case("comma") {
        return Some(AppCommand::OpenPreferences);
    }
    match font_command(name, key_char, true)? {
        FontCommand::Increase => Some(AppCommand::IncreaseFont),
        FontCommand::Decrease => Some(AppCommand::DecreaseFont),
        FontCommand::Reset => Some(AppCommand::ResetFont),
    }
}

/// Whether a keystroke is claimed by the host rather than the terminal.
#[must_use]
pub fn is_app_shortcut_with_char(name: &str, key_char: Option<&str>, mods: Modifiers) -> bool {
    app_command(name, key_char, mods).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_commands_cover_every_platform_spelling() {
        assert_eq!(font_command("=", None, true), Some(FontCommand::Increase));
        assert_eq!(font_command("-", None, true), Some(FontCommand::Decrease));
        assert_eq!(font_command("0", None, true), Some(FontCommand::Reset));
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
        assert_eq!(
            font_command("unknown", Some("-"), true),
            Some(FontCommand::Decrease)
        );
        assert_eq!(
            font_command("unknown", Some("+"), true),
            Some(FontCommand::Increase)
        );
        assert_eq!(font_command("-", None, false), None);
        assert_eq!(font_command("a", Some("a"), true), None);
    }

    #[test]
    fn app_commands_resolve_for_every_binding_spelling() {
        let ctrl = Modifiers::new(false, true, false, false);
        let ctrl_shift = Modifiers::new(true, true, false, false);
        assert_eq!(
            app_command("c", Some("C"), ctrl_shift),
            Some(AppCommand::CopySelection)
        );
        assert_eq!(
            app_command("v", None, ctrl_shift),
            Some(AppCommand::PasteClipboard)
        );
        assert_eq!(
            app_command("a", None, ctrl_shift),
            Some(AppCommand::SelectAll)
        );
        assert_eq!(
            app_command("insert", None, Modifiers::new(true, false, false, false)),
            Some(AppCommand::PasteClipboard)
        );
        assert_eq!(
            app_command(",", None, ctrl),
            Some(AppCommand::OpenPreferences)
        );
        assert_eq!(
            app_command("comma", None, ctrl),
            Some(AppCommand::OpenPreferences)
        );
        assert_eq!(app_command("=", None, ctrl), Some(AppCommand::IncreaseFont));
        assert_eq!(
            app_command("minus", None, ctrl),
            Some(AppCommand::DecreaseFont)
        );
        assert_eq!(app_command("0", None, ctrl), Some(AppCommand::ResetFont));
        assert_eq!(
            app_command("c", None, Modifiers::new(false, true, true, false)),
            None
        );
        assert_eq!(app_command("c", None, ctrl), None);
    }

    #[test]
    fn shortcut_allowlist_has_one_source_of_truth() {
        let ctrl_shift = Modifiers::new(true, true, false, false);
        assert!(is_app_shortcut_with_char("c", Some("C"), ctrl_shift));
        assert!(!is_app_shortcut_with_char(
            "c",
            None,
            Modifiers::new(false, true, true, false)
        ));
        assert!(!is_app_shortcut_with_char("c", None, Modifiers::default()));
    }
}
