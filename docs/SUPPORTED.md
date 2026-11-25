# Supported and Unsupported Features

This list is part of the M7 release contract. It prevents a binary from
claiming capabilities inherited from Ghostty that this renderer does not
implement.

## Supported

- One window with one terminal session and one shell on Windows and Linux.
- Windows: OS ConPTY. Linux: POSIX PTY with `$SHELL` or `/bin/sh`.
- Shell override from the command line, no settings UI.
- Terminal output: ANSI colors, 256-color and true color, bold/italic/underline,
  alternate screen, cursor styles and resize/reflow via Ghostty VT.
- Keyboard and text: Ghostty's mode-synced key encoder, GPUI committed-text and
  IME bridge, bracketed paste.
- Mouse: local character/word/line selection, copy, TUI mouse reporting with an
  explicit escape modifier (platform/Super key) for local selection.
- Wheel and trackpad scrolling through Ghostty's viewport, plus wheel reports
  to mouse-reporting TUIs.
- Bounded scrollback configured at session creation; a scrollbar view driven by
  Ghostty's scrollbar state.
- Window title follows OSC 0/2 with a `pain` fallback.
- Font size shortcuts `Ctrl+=`, `Ctrl+-`, `Ctrl+0`.
- Startup/runtime errors appended to a log file.

## Deliberately unsupported

- macOS binary, and any macOS code presented as supported.
- Tabs, splits, panes, detachable sessions, profiles, workspaces, session
  restore, command palette, global hotkeys or quick terminal.
- Graphical preferences, theme marketplace, plugins or auto-update.
- Kitty graphics, Sixel, image protocols, shaders and ligature configuration.
- Shell integration installation, semantic prompt navigation, notifications,
  hyperlinks and OSC clipboard program access.
- Multiple terminal sessions in one process, remote control or SSH client.
- `TERM=xterm-ghostty`: the app advertises the conservative
  `xterm-256color` plus `COLORTERM=truecolor` identity instead.
- Idle scrollback compression: omitted until profiling shows a need that
  outweighs its serialization and foreground-stall risk.

## Known environment caveats

- WSLg containers without a CJK font show tofu boxes for wide CJK glyphs;
  Ghostty's grapheme and two-cell geometry remain correct.
- Under WSLg, MESA-EGL/dzn Vulkan warnings can appear in the host log; no app
  panic was observed.
- The Windows GUI build writes diagnostics only to the log file, not a console.
