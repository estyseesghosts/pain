//! Terminal colour palettes.
//!
//! The UI theme (Aqua/Geist, light/dark) is Silvestris's responsibility. This
//! module only describes the *terminal* colours: default foreground,
//! background, cursor and the base-16 ANSI entries. Ghostty generates the
//! remaining 240 palette entries from those base colours through its own
//! `ghostty_color_palette_generate` API, so no second colour engine lives here.

use serde::{Deserialize, Serialize};

/// User-facing palette choice, persisted in the preferences document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaletteChoice {
    /// Ghostty's built-in default palette: preserves the pre-preferences look.
    #[default]
    Ghostty,
    /// Follow the UI theme's light/dark appearance.
    Theme,
    CatppuccinMocha,
    GruvboxDark,
    SolarizedLight,
}

impl PaletteChoice {
    pub const ALL: [Self; 5] = [
        Self::Ghostty,
        Self::Theme,
        Self::CatppuccinMocha,
        Self::GruvboxDark,
        Self::SolarizedLight,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ghostty => "ghostty",
            Self::Theme => "theme",
            Self::CatppuccinMocha => "catppuccin-mocha",
            Self::GruvboxDark => "gruvbox-dark",
            Self::SolarizedLight => "solarized-light",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ghostty => "Default",
            Self::Theme => "Theme",
            Self::CatppuccinMocha => "Catppuccin",
            Self::GruvboxDark => "Gruvbox",
            Self::SolarizedLight => "Solarized",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|choice| choice.as_str() == value)
            .unwrap_or_default()
    }
}

/// An RGB triplet. `hex` takes the usual `0xRRGGBB` form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn hex(value: u32) -> Self {
        Self {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        }
    }
}

/// The colours handed to Ghostty. `dark` also drives the terminal's answer to
/// a `CSI ? 996 n` colour-scheme query, so applications agree with what they see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    /// ANSI indices 0..16; Ghostty fills 16..256.
    pub base: [Rgb; 16],
    pub dark: bool,
}

const fn base(colors: [u32; 16]) -> [Rgb; 16] {
    let mut out = [Rgb::hex(0); 16];
    let mut i = 0;
    while i < 16 {
        out[i] = Rgb::hex(colors[i]);
        i += 1;
    }
    out
}

/// Resolve a palette choice for the active UI appearance. `None` means keep
/// Ghostty's built-in palette; fixed palettes ignore the UI appearance and
/// `Theme` follows light/dark.
#[must_use]
pub fn resolve(choice: PaletteChoice, dark: bool) -> Option<Palette> {
    if choice == PaletteChoice::Ghostty {
        return None;
    }
    Some(match choice {
        PaletteChoice::Ghostty => unreachable!("handled before the match"),
        PaletteChoice::Theme if dark => Palette {
            background: Rgb::hex(0x191919),
            foreground: Rgb::hex(0xececec),
            cursor: Rgb::hex(0x0095ff),
            base: base([
                0x191919, 0xe34646, 0x279447, 0xd99327, 0x0095ff, 0xb45acb, 0x2aa198, 0xececec,
                0x5c5c5c, 0xff6b6b, 0x5ddb7a, 0xffc857, 0x5cb3ff, 0xd487f0, 0x5ee0d0, 0xffffff,
            ]),
            dark: true,
        },
        PaletteChoice::Theme => Palette {
            background: Rgb::hex(0xffffff),
            foreground: Rgb::hex(0x1a1a1a),
            cursor: Rgb::hex(0x0095ff),
            base: base([
                0x1a1a1a, 0xc03b32, 0x2a7d3a, 0x9a6b12, 0x006fc4, 0x8d4fa8, 0x187f7a, 0x4d4d4d,
                0x808080, 0xe34646, 0x279447, 0xd99327, 0x0095ff, 0xb45acb, 0x2aa198, 0x1a1a1a,
            ]),
            dark: false,
        },
        PaletteChoice::CatppuccinMocha => Palette {
            background: Rgb::hex(0x1e1e2e),
            foreground: Rgb::hex(0xcdd6f4),
            cursor: Rgb::hex(0xf5e0dc),
            base: base([
                0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
                0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8,
            ]),
            dark: true,
        },
        PaletteChoice::GruvboxDark => Palette {
            background: Rgb::hex(0x282828),
            foreground: Rgb::hex(0xebdbb2),
            cursor: Rgb::hex(0xfe8019),
            base: base([
                0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
                0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
            ]),
            dark: true,
        },
        PaletteChoice::SolarizedLight => Palette {
            background: Rgb::hex(0xfdf6e3),
            foreground: Rgb::hex(0x657b83),
            cursor: Rgb::hex(0x268bd2),
            base: base([
                0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
                0x002b36, 0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
            ]),
            dark: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parses_hex_components() {
        assert_eq!(
            Rgb::hex(0x0a1b2c),
            Rgb {
                r: 10,
                g: 27,
                b: 44
            }
        );
    }

    #[test]
    fn every_choice_resolves_to_a_complete_base_palette() {
        for choice in PaletteChoice::ALL {
            for dark in [true, false] {
                let Some(resolved) = resolve(choice, dark) else {
                    assert_eq!(choice, PaletteChoice::Ghostty);
                    continue;
                };
                assert_eq!(resolved.base.len(), 16);
                assert!(
                    resolved.base.iter().all(|c| *c != Rgb::hex(0)),
                    "{choice:?} has an unset base colour"
                );
                let expected_dark = match choice {
                    PaletteChoice::Theme => dark,
                    PaletteChoice::SolarizedLight => false,
                    _ => true,
                };
                assert_eq!(resolved.dark, expected_dark);
            }
        }
    }

    #[test]
    fn fixed_palettes_ignore_the_ui_appearance() {
        for choice in [
            PaletteChoice::CatppuccinMocha,
            PaletteChoice::GruvboxDark,
            PaletteChoice::SolarizedLight,
        ] {
            assert_eq!(resolve(choice, true), resolve(choice, false));
        }
        assert_ne!(
            resolve(PaletteChoice::Theme, true),
            resolve(PaletteChoice::Theme, false)
        );
    }

    #[test]
    fn palette_names_round_trip() {
        for choice in PaletteChoice::ALL {
            assert_eq!(PaletteChoice::parse(choice.as_str()), choice);
        }
        assert_eq!(PaletteChoice::parse("unknown"), PaletteChoice::Ghostty);
    }
}
