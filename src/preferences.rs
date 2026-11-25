//! One-window preferences for appearance and terminal colours.
//!
//! The form follows Chaus's simple single-window pattern: Silvestris's
//! `WindowFrame` + `Form`/`FormRow` + `SegmentedControl`, with a versioned JSON
//! document owned by `silvestris::prefs::Preferences`. Edits apply in memory
//! immediately (both windows repaint) and write through to disk without
//! touching the shell or its PTY.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, px, size, App, AppContext as _, Bounds, Context, Entity, Global, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, WindowHandle,
};
use serde::{Deserialize, Serialize};
use silvestris::chrome::{fixed_window_options, open_window, WindowFrame};
use silvestris::controls::{Button, PopUpButton, Segment, SegmentedControl};
use silvestris::core::prefs::{Loaded, Versioned};
use silvestris::preferences::{Form, FormRow};
use silvestris::prefs::Preferences;
use silvestris::{set_theme_preference, Appearance, ThemeId, ThemePreference};

use crate::log::{data_dir, error};
use crate::platform::profiles::{self, ShellProfile};
use crate::terminal::palette::{self, PaletteChoice};
use crate::terminal::session::TerminalSession;

const SCHEMA_VERSION: u32 = 1;

/// The four Silvestris themes exposed by the preferences window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeChoice {
    AquaDark,
    #[default]
    AquaLight,
    GeistDark,
    GeistLight,
}

impl ThemeChoice {
    pub const ALL: [Self; 4] = [
        Self::AquaDark,
        Self::AquaLight,
        Self::GeistDark,
        Self::GeistLight,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AquaDark => "aqua-dark",
            Self::AquaLight => "aqua-light",
            Self::GeistDark => "geist-dark",
            Self::GeistLight => "geist-light",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AquaDark => "Aqua Dark",
            Self::AquaLight => "Aqua Light",
            Self::GeistDark => "Geist Dark",
            Self::GeistLight => "Geist Light",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|choice| choice.as_str() == value)
            .unwrap_or_default()
    }

    #[must_use]
    pub fn theme_id(self) -> ThemeId {
        match self {
            Self::AquaDark => ThemeId::AQUA_DARK,
            Self::AquaLight => ThemeId::AQUA_LIGHT,
            Self::GeistDark => ThemeId::GEIST_DARK,
            Self::GeistLight => ThemeId::GEIST_LIGHT,
        }
    }

    #[must_use]
    pub fn appearance(self) -> Appearance {
        self.theme_id().appearance
    }
}

/// Persisted, versioned preferences document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiPreferences {
    #[serde(default = "schema")]
    pub schema_version: u32,
    #[serde(default)]
    pub theme: ThemeChoice,
    #[serde(default)]
    pub palette: PaletteChoice,
    /// Saved shell profile id. `None` means "use the platform default".
    #[serde(default)]
    pub profile: Option<String>,
}

fn schema() -> u32 {
    SCHEMA_VERSION
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            theme: ThemeChoice::default(),
            palette: PaletteChoice::default(),
            profile: None,
        }
    }
}

impl Versioned for UiPreferences {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn set_schema_version(&mut self, version: u32) {
        self.schema_version = version;
    }
}

/// Path of the persisted document, beside the log in the per-user data dir.
#[must_use]
pub fn preferences_path() -> PathBuf {
    data_dir().join("preferences.json")
}

/// Entity other views can read and controls can update.
pub struct PreferencesGlobal(pub Entity<Preferences<UiPreferences>>);

impl Global for PreferencesGlobal {}

/// The live session, so a palette change can be applied without reopening it.
pub struct SessionGlobal(pub Arc<TerminalSession>);

impl Global for SessionGlobal {}

/// Profiles detected once at startup. Preferences only reads this list; a
/// change of default affects the next launch or the next jump-list window.
pub struct ShellProfilesGlobal(pub Vec<ShellProfile>);

impl Global for ShellProfilesGlobal {}

/// Open the document once and install it as a global. A malformed file keeps
/// defaults and is left untouched so a newer version can still read it.
pub fn install(cx: &mut App) {
    let path = preferences_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (preferences, loaded) = Preferences::<UiPreferences>::open(path.clone(), &[], cx);
    if let Loaded::Malformed { error: reason, .. } = &loaded {
        error(
            "preferences",
            &format!("{}: {reason}; using defaults", path.display()),
        );
    }
    cx.set_global(PreferencesGlobal(preferences));
}

/// Apply the current document to the UI theme and the live terminal.
pub fn apply(cx: &mut App) {
    let value = cx.global::<PreferencesGlobal>().0.read(cx).value().clone();
    set_theme_preference(ThemePreference::explicit(value.theme.theme_id()), cx);
    let colors = palette::resolve(value.palette, value.theme.appearance() == Appearance::Dark);
    if let Some(session) = cx.try_global::<SessionGlobal>() {
        if let Err(failure) = session.0.set_colors(colors.as_ref()) {
            error(
                "preferences",
                &format!("could not apply palette: {failure}"),
            );
        }
    }
    cx.refresh_windows();
}

/// One preferences window per app. A stale handle is replaced when closed.
struct PreferencesWindow(Option<WindowHandle<PreferencesView>>);

impl Global for PreferencesWindow {}

/// Open (or focus) the single preferences window.
pub fn open_preferences(cx: &mut App) {
    if let Some(handle) = cx
        .try_global::<PreferencesWindow>()
        .and_then(|global| global.0)
    {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(600.), px(380.)), cx);
    let options = fixed_window_options("Preferences", bounds, cx);
    match open_window(options, cx, |window, cx| {
        cx.new(|cx| PreferencesView::new(window, cx))
    }) {
        Ok(handle) => cx.set_global(PreferencesWindow(Some(handle))),
        Err(failure) => error(
            "preferences",
            &format!("could not open preferences: {failure:#}"),
        ),
    }
}

struct PreferencesView;

impl PreferencesView {
    fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for PreferencesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let value = cx.global::<PreferencesGlobal>().0.read(cx).value().clone();
        let profiles = cx
            .try_global::<ShellProfilesGlobal>()
            .map(|global| global.0.clone())
            .unwrap_or_default();
        let automatic = profiles::default_profile_id(&profiles)
            .and_then(|id| profiles::find_profile(&profiles, id))
            .map(|profile| profile.label.as_str())
            .unwrap_or("system shell");
        let labels: Vec<SharedString> =
            std::iter::once(SharedString::from(format!("Automatic — {automatic}")))
                .chain(
                    profiles
                        .iter()
                        .map(|profile| SharedString::from(profile.label.clone())),
                )
                .collect();
        let mut profile_choices: Vec<Option<String>> = Vec::with_capacity(profiles.len() + 1);
        profile_choices.push(None);
        profile_choices.extend(profiles.iter().map(|profile| Some(profile.id.clone())));
        let selected_profile = value
            .profile
            .as_ref()
            .and_then(|id| profiles.iter().position(|profile| &profile.id == id))
            .map(|index| index + 1)
            .unwrap_or(0);
        let profile_picker = PopUpButton::new("preferences-profile", "Shell profile", labels)
            .selected(Some(selected_profile))
            .on_choose(move |index, _, cx| {
                let choice = profile_choices.get(*index).cloned().flatten();
                let preferences = cx.global::<PreferencesGlobal>().0.clone();
                preferences.update(cx, |document, cx| {
                    if let Err(failure) =
                        document.set("profile", |value| value.profile = choice.clone(), cx)
                    {
                        error(
                            "preferences",
                            &format!("could not save shell profile: {failure}"),
                        );
                    }
                });
                cx.refresh_windows();
            });

        let theme = SegmentedControl::new(
            "preferences-theme",
            "Theme",
            ThemeChoice::ALL
                .into_iter()
                .map(|choice| Segment::new(choice.as_str(), choice.label()))
                .collect(),
        )
        .selected(value.theme.as_str())
        .on_select(move |id, _, cx| {
            let theme = ThemeChoice::parse(id.as_ref());
            let preferences = cx.global::<PreferencesGlobal>().0.clone();
            preferences.update(cx, |document, cx| {
                if let Err(failure) = document.set("theme", |value| value.theme = theme, cx) {
                    error("preferences", &format!("could not save theme: {failure}"));
                }
            });
            apply(cx);
        });

        let palette = SegmentedControl::new(
            "preferences-palette",
            "Terminal palette",
            PaletteChoice::ALL
                .into_iter()
                .map(|choice| Segment::new(choice.as_str(), choice.label()))
                .collect(),
        )
        .selected(value.palette.as_str())
        .on_select(move |id, _, cx| {
            let choice = PaletteChoice::parse(id.as_ref());
            let preferences = cx.global::<PreferencesGlobal>().0.clone();
            preferences.update(cx, |document, cx| {
                if let Err(failure) = document.set("palette", |value| value.palette = choice, cx) {
                    error("preferences", &format!("could not save palette: {failure}"));
                }
            });
            apply(cx);
        });

        #[cfg(target_os = "windows")]
        let profile_hint = "Used when a new pain process starts. Automatic follows \
                            PowerShell 7, then Windows PowerShell, then the command \
                            interpreter.";
        #[cfg(not(target_os = "windows"))]
        let profile_hint = "Used when a new pain process starts. Automatic uses the login shell.";
        let hint = "Theme sets the window look. The palette chooses ANSI colours; \
                    Default keeps Ghostty's palette and Theme follows light/dark.";

        WindowFrame::new("Preferences").fixed_size().child(
            div()
                .size_full()
                .p(px(20.0))
                .flex()
                .flex_col()
                .gap(px(14.0))
                .child(
                    Form::new(130.0)
                        .row(FormRow::new("Shell profile", profile_picker).note(profile_hint))
                        .row(FormRow::new("Theme", theme))
                        .row(FormRow::new("Terminal palette", palette).note(hint)),
                )
                .child(
                    div().flex().justify_end().child(
                        Button::new("preferences-close", "Close")
                            .on_click(|_, window, _| window.remove_window()),
                    ),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_choices_map_to_silvestris_ids() {
        assert_eq!(ThemeChoice::AquaDark.theme_id(), ThemeId::AQUA_DARK);
        assert_eq!(ThemeChoice::AquaLight.theme_id(), ThemeId::AQUA_LIGHT);
        assert_eq!(ThemeChoice::GeistDark.theme_id(), ThemeId::GEIST_DARK);
        assert_eq!(ThemeChoice::GeistLight.theme_id(), ThemeId::GEIST_LIGHT);
        assert_eq!(ThemeChoice::GeistDark.appearance(), Appearance::Dark);
        assert_eq!(ThemeChoice::GeistLight.appearance(), Appearance::Light);
    }

    #[test]
    fn theme_names_round_trip() {
        for choice in ThemeChoice::ALL {
            assert_eq!(ThemeChoice::parse(choice.as_str()), choice);
        }
        assert_eq!(ThemeChoice::parse("unknown"), ThemeChoice::AquaLight);
    }

    #[test]
    fn default_document_preserves_the_previous_look() {
        let value = UiPreferences::default();
        assert_eq!(value.schema_version, SCHEMA_VERSION);
        assert_eq!(value.theme, ThemeChoice::AquaLight);
        assert_eq!(value.palette, PaletteChoice::Ghostty);
    }
}
