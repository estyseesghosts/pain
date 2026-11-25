# Release Smoke Checklist

Automated coverage: `scripts/smoke.ps1` (Windows) and `scripts/smoke.sh`
(Linux/WSLg) run the headless suite, the M6 stress counters and a release build
by default. They never synthesize input or foreground a window. The launch check
is opt-in: pass `-Gui` on Windows or `--gui` on Linux only when you are not
using the desktop at that moment.

Run the manual half on each platform before calling a build shippable.

## Both platforms

1. Launch with no arguments. Exactly one titled window opens; no empty console
   window appears on Windows.
2. The default shell starts. Type a command and press Enter; output appears.
3. `Ctrl+=`, `Ctrl+-` and `Ctrl+0` grow, shrink and reset the font. These are
   element-level GPUI actions, not global handlers; if one is dead, check the
   log before moving it back to `App::on_action`.
4. Run `vim`, enter insert mode, type text, leave and resize the window. The
   content reflows and the TUI remains usable.
5. Produce more than a screen of output. Wheel up scrolls history, wheel down
   returns to the active area; the thumb on the right follows.
6. Drag select text, double-click a word, triple-click a line, then
   `Ctrl+Shift+C`. Paste with both `Ctrl+Shift+V` and `Shift+Insert` inside the
   shell and confirm the text arrives as one bracketed paste, with no extra
   escape sequence reaching the shell after the copy.
7. `Ctrl+Shift+A` selects all; copying it yields the whole buffer.
8. In `vim` (or a TUI requesting mode 1003/any-motion) with mouse enabled:
   wheel and drag are reported to the child, hovering moves the cursor, and a
   drag that leaves the surface keeps reporting. Holding Shift (or Super)
   keeps selection local instead. Right-click with reporting off is left to the
   platform context menu.
9. `Ctrl+,` opens Preferences. Choose each of Aqua Light/Dark and Geist
   Light/Dark: the window chrome and terminal both follow, and text stays
   readable. Choose `Theme`, `Catppuccin`, `Gruvbox`, `Solarized` and `Default`
   in turn; the ANSI colours change live and the shell keeps running. Reopen
   Preferences with `Ctrl+,`; the last choices persist.
10. In Preferences, change **Shell profile** to each detected profile. The
    running shell must not restart; a newly launched pain process must start
    the selected shell, and `Automatic` must fall back to the platform default.
11. Set a title with `printf '\033]0;smoke title\007'` and confirm the window
    title changes. Clear it and confirm the `pain` fallback returns.
12. Close the terminal window with Preferences still open: the shell exits but
    Preferences stays usable. Close Preferences last. No process is left behind
    and the log contains no new ERROR lines.

## Windows only

- Launch `pain-release.exe` from Explorer: no dangling console.
- Right-click the running pain taskbar button. Under **Tasks**, every detected
  profile appears as `New <profile> window`; choosing one starts that shell in
  a new process. The same list should appear after pinning pain.
- `pain --profile pwsh7` starts PowerShell 7. `pain --profile wsl-<distro>`
  starts that WSL distribution in its home directory. An unknown id logs an
  error and falls back to the default.
- With no saved preference, the default is PowerShell 7 when installed, then
  Windows PowerShell, then `cmd.exe`.
- `pain --shell powershell.exe -NoLogo` starts PowerShell instead of the saved
  profile; `--shell` wins over `--profile`.
- `pain --version` and `--help` write to the log file (GUI binary has no
  console).
- Rebuilding does not require closing a previously launched binary; if Cargo
  reports a locked `pain.exe`, close the old window first.

## Linux / WSLg only

- Run from the WSL ext4 working copy with `DISPLAY`, `WAYLAND_DISPLAY` and
  `XDG_RUNTIME_DIR` exported.
- `pain -- /bin/bash -i` starts the requested shell.
- Resize the WSLg window repeatedly; no PTY resize spam appears in the log and
  the shell prompt stays at the right geometry.
- CJK glyphs may be tofu boxes if the container has no CJK font; the cells must
  still occupy two columns.

## Recording a result

Copy the `M6 ...` lines printed by `scripts/smoke.ps1` / `smoke.sh` into
[docs/PERFORMANCE.md](PERFORMANCE.md) when changing hot paths, and note the
platform, profile, terminal size and workload as described there.
