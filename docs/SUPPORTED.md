# Supported and Unsupported Features

This list is part of the M7 release contract. It prevents a binary from
claiming capabilities inherited from Ghostty that this renderer does not
implement.

## Supported

- One window with one terminal session and one shell on Windows and Linux.
- Windows: ConPTY through the bundled, pinned OpenConsole `conpty.dll` when it
  is packaged beside `pain.exe`; the in-box `kernel32` host is a documented
  Kitty-graphics-degraded fallback. Linux: POSIX PTY with `$SHELL` or
  `/bin/sh`.
- Shell profiles on Windows: PowerShell 7, Windows PowerShell, cmd, Git Bash,
  devkitPro MSYS2 Bash and each registered WSL distribution. The default is
  chosen in Preferences; the same list appears in the taskbar jump list.
- Shell override from the command line (`--shell`) or a specific profile
  (`--profile`).
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
- Kitty graphics: inline direct transmission with raw RGB/RGBA/gray/gray+alpha
  and PNG (bounded, process-global decoder), transmit+display and
  transmit-then-place, placement IDs and offsets, source rectangles, explicit
  rows/columns, alpha, positive/negative `z`, Ghostty's below-background,
  below-text and above-text layers, fixed, relative and Unicode-placeholder
  placements, scrollback movement, clipping, resize/reflow, alternate screen,
  animation frames, stop/resume, and bounded storage-limit eviction. Kitty
  textures are cached by `(protocol, image_id, generation)` and dropped from
  the GPUI atlas when superseded.
- Kitty transport policy: the arbitrary-file medium is implemented but disabled
  by default; the temporary-file medium requires an explicit allowed directory;
  shared memory is disabled where the host namespace does not apply. Direct
  inline transfer is the portable default. Storage caps remain finite and a
  zero cap disables graphics.
- Window title follows OSC 0/2 with a `pain` fallback.
- Font size shortcuts `Ctrl+=`, `Ctrl+-`, `Ctrl+0`; `Ctrl+,` opens the
  one-window Preferences form.
- Startup/runtime errors appended to a log file.

## Deliberately unsupported

- macOS binary, and any macOS code presented as supported.
- Tabs, splits, panes, detachable sessions, workspaces, session restore,
  command palette, global hotkeys or quick terminal.
- Ghostty-style window/session profiles. Pain's profile setting only chooses
  the shell command; it does not save window layouts or per-profile fonts.
- Multi-pane settings, a theme marketplace, plugins or auto-update.
- Sixel, shaders and ligature configuration. Kitty graphics is supported, but
  Sixel is not decoded or painted.
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
