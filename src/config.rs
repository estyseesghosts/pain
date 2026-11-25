//! Small command-line surface for the release build: shell, font size and
//! scrollback caps only. There is deliberately no settings UI and no config
//! file discovery at startup (docs/PERFORMANCE.md, "Startup").

use crate::platform::ShellCommand;
use crate::terminal::ghostty::version as ghostty_version;

pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub const DEFAULT_SCROLLBACK_BYTES: usize = 16 * 1024 * 1024;

/// Parsed essentials. `font_size` of `None` means "use the theme default".
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub shell: Option<ShellCommand>,
    pub font_size: Option<f32>,
    pub scrollback_lines: usize,
    pub scrollback_bytes: usize,
    pub help: bool,
    pub version: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: None,
            font_size: None,
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            scrollback_bytes: DEFAULT_SCROLLBACK_BYTES,
            help: false,
            version: false,
        }
    }
}

/// Parse `argv`, returning a human-readable error for bad input rather than
/// silently falling back to defaults.
pub fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut config = Config::default();
    let mut index = 1usize;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--" => {
                let mut rest = args[index + 1..].iter();
                if let Some(program) = rest.next() {
                    if !program.is_empty() {
                        config.shell = Some(ShellCommand {
                            program: program.clone(),
                            args: rest.cloned().collect(),
                        });
                    }
                }
                break;
            }
            "--shell" => {
                let program = args
                    .get(index + 1)
                    .ok_or_else(|| "--shell needs a program".to_owned())?;
                if program.is_empty() {
                    return Err("--shell needs a non-empty program".to_owned());
                }
                let mut end = index + 2;
                while end < args.len() && !is_app_option(&args[end]) {
                    end += 1;
                }
                config.shell = Some(ShellCommand {
                    program: program.clone(),
                    args: args[index + 2..end].to_vec(),
                });
                index = end;
            }
            "--font-size" => {
                config.font_size = Some(parse_value(args, &mut index, "--font-size", |text| {
                    text.parse::<f32>()
                        .ok()
                        .filter(|size| size.is_finite() && *size > 0.0)
                })?);
            }
            "--scrollback-lines" => {
                config.scrollback_lines =
                    parse_value(args, &mut index, "--scrollback-lines", |text| {
                        text.parse::<usize>().ok()
                    })?;
            }
            "--scrollback-bytes" => {
                config.scrollback_bytes =
                    parse_value(args, &mut index, "--scrollback-bytes", |text| {
                        text.parse::<usize>().ok()
                    })?;
            }
            "--help" | "-h" => {
                config.help = true;
                index += 1;
            }
            "--version" | "-V" => {
                config.version = true;
                index += 1;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(config)
}

fn is_app_option(arg: &str) -> bool {
    matches!(
        arg,
        "--shell"
            | "--"
            | "--font-size"
            | "--scrollback-lines"
            | "--scrollback-bytes"
            | "--help"
            | "-h"
            | "--version"
            | "-V"
    )
}

fn parse_value<T>(
    args: &[String],
    index: &mut usize,
    option: &str,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<T, String> {
    let text = args
        .get(*index + 1)
        .ok_or_else(|| format!("{option} needs a value"))?;
    let value = parse(text).ok_or_else(|| format!("invalid value for {option}: {text}"))?;
    *index += 2;
    Ok(value)
}

/// Usage text printed for `--help` and for a parse error.
#[must_use]
pub fn usage() -> &'static str {
    "usage: pain [--shell <program> [args...]] [-- <program> [args...]]\n\
     \x20      [--font-size <pixels>] [--scrollback-lines <n>]\n\
     \x20      [--scrollback-bytes <n>] [--version] [--help]\n\
     \n\
     Ctrl+= / Ctrl+- / Ctrl+0 change font size; Ctrl+Shift+C/V/A copy, paste\n\
     and select all. Wheel scrolls history unless a TUI owns mouse reporting."
}

/// Version/source attribution for the bundled terminal engine.
#[must_use]
pub fn version_line() -> String {
    format!(
        "pain {} (libghostty-vt {})",
        env!("CARGO_PKG_VERSION"),
        ghostty_version()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        std::iter::once("pain")
            .chain(items.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn essentials_parse_shell_font_and_scrollback() {
        let config = parse_args(&argv(&[
            "--shell",
            "powershell.exe",
            "-NoLogo",
            "--font-size",
            "15.5",
            "--scrollback-lines",
            "20000",
            "--scrollback-bytes",
            "33554432",
        ]))
        .expect("valid config");
        assert_eq!(
            config.shell,
            Some(ShellCommand {
                program: "powershell.exe".into(),
                args: vec!["-NoLogo".into()],
            })
        );
        assert_eq!(config.font_size, Some(15.5));
        assert_eq!(config.scrollback_lines, 20_000);
        assert_eq!(config.scrollback_bytes, 32 * 1024 * 1024);
    }

    #[test]
    fn bare_double_dash_is_the_shell_command() {
        let config = parse_args(&argv(&["--", "/bin/bash", "-i"])).expect("valid config");
        assert_eq!(config.shell.expect("shell").program, "/bin/bash");
        assert_eq!(config.font_size, None);
        assert_eq!(config.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
    }

    #[test]
    fn bad_values_are_errors_not_silent_defaults() {
        assert!(parse_args(&argv(&["--font-size"])).is_err());
        assert!(parse_args(&argv(&["--font-size", "abc"])).is_err());
        assert!(parse_args(&argv(&["--font-size", "-1"])).is_err());
        assert!(parse_args(&argv(&["--scrollback-lines", "x"])).is_err());
        assert!(parse_args(&argv(&["--nope"])).is_err());
    }

    #[test]
    fn version_line_attributes_libghostty_vt() {
        let line = version_line();
        assert!(line.starts_with("pain "));
        assert!(line.contains("libghostty-vt"));
    }
}
