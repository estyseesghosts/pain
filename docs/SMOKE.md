# Release Smoke Checklist

Automated coverage: `scripts/smoke.ps1` (Windows) and `scripts/smoke.sh`
(Linux/WSLg) run the headless suite, the M6 stress counters, a release build and
a one-window launch/close check. They never synthesize input on your desktop.

Run the manual half on each platform before calling a build shippable.

## Both platforms

1. Launch with no arguments. Exactly one titled window opens; no empty console
   window appears on Windows.
2. The default shell starts. Type a command and press Enter; output appears.
3. `Ctrl+=`, `Ctrl+-` and `Ctrl+0` grow, shrink and reset the font.
4. Run `vim`, enter insert mode, type text, leave and resize the window. The
   content reflows and the TUI remains usable.
5. Produce more than a screen of output. Wheel up scrolls history, wheel down
   returns to the active area; the thumb on the right follows.
6. Drag select text, double-click a word, triple-click a line, then
   `Ctrl+Shift+C`. Paste with `Ctrl+Shift+V` inside the shell and confirm the
   text arrives as one bracketed paste.
7. `Ctrl+Shift+A` selects all; copying it yields the whole buffer.
8. In `vim` with mouse mode enabled, wheel and drag are reported to the child;
   holding the platform/Super key keeps selection local.
9. Set a title with `printf '\033]0;smoke title\007'` and confirm the window
   title changes. Clear it and confirm the `pain` fallback returns.
10. Close the window. The child shell exits, no process is left behind, and the
    configured log file contains no new ERROR lines.

## Windows only

- Launch `pain-release.exe` from Explorer: no dangling console.
- `pain --shell powershell.exe -NoLogo` starts PowerShell instead of `cmd.exe`.
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
