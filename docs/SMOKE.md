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

## Repair-pass regression checks (add to the manual half)

- Horizontal-only drag: rows stay fixed while columns cross boundaries; the
  shell's `stty size`, rendered grid, mouse hit-testing and selection agree.
- Font-size change at an unchanged grid: Ghostty pixel dimensions change, PTY
  grid resize counter does not. Moving the window between different-DPI
  monitors behaves identically at the same logical size.
- Scrollbar thumb drag released inside the terminal, outside the window, over
  the titlebar and over the traffic lights: after each, the next mouse move
  must not move the thumb, and close/minimize/zoom still work.
- `Ctrl+C` sends ETX; `Ctrl+V` stays quoted-insert; `Ctrl+Shift+C/V/A`,
  `Shift+Insert`, `Ctrl+,` and the font keys each fire once with no stray
  escape sequence after a copy. AltGr-composed text types normally.
- With a Japanese/Chinese/Korean IME, intermediate readings never appear at
  the shell prompt; committing with Enter/Space inserts the chosen candidate
  once; Escape cancels; candidate placement follows the terminal cursor.
- A TUI enabling Kitty `report_events` shows press → repeat → release; app
  shortcuts produce no child release.
- A mode-1003 TUI under high-rate pointer motion with simultaneous output
  keeps reporting without growing the writer queue.
- Cursor blinking repaints only the cursor quad; shaped rows are reused.
- OSC titles A/B/C arriving before a UI frame end with C applied to both the
  native title and the drawn Silvestris title band.

## Mouse interaction checklist (required manual pass)

Automated geometry tests pin the coordinate contract, but only a real pointer
can establish that selection and reporting *feel* aligned. Walk this list on
each platform after any pointer change; do not mark it green from `cargo test`.

1. **Local click and drag selection** with reporting off: click at a cell,
   drag across rows and columns, release, and copy. The copied characters must
   match the cells under the pointer, including the first and last cell and the
   unused remainder strip at the right/bottom edge.
2. **Shift/Super override** while a mode-1003 TUI has mouse tracking on: the
   same drag must select local text instead of reaching the child, and releasing
   the modifier must restore reporting immediately.
3. **Press and release in `vim` or another mouse-reporting TUI**: clicks land on
   the expected character; a press followed by a release produces exactly one
   press and one release in the child, with no duplicate or missing event.
4. **Motion and drag reporting**: hovering produces any-motion reports in 1003
   and none in 1000/1002; a drag that leaves the surface keeps reporting while
   a button is held and stops when it is not.
5. **Wheel reporting**: wheel up/down inside a mouse-reporting TUI is reported
   as buttons 4/5 one per whole row; with reporting off it moves Ghostty's
   viewport, and the scrollbar thumb follows.
6. **Scrollbar dragging**: drag the thumb, release inside the terminal, outside
   the window, over the titlebar and over the traffic lights. The next mouse
   move must not move the thumb, and window buttons still work.
7. **Window movement and live resize**: dragging the terminal window and
   resizing it horizontally, vertically and both at once keeps hit-testing,
   selection and the TUI's `stty size` in agreement with the rendered grid.
8. **Font-size change** (`Ctrl+=`, `Ctrl+-`, `Ctrl+0`) at an unchanged grid:
   device cell pixels change, the PTY grid resize count does not, and the next
   click still lands on the cell under the pointer.
9. **HiDPI / non-100% scale** (125%, 150%, 200% where available): repeat steps
   1–5 at fractional metrics such as `9.7 × 18.4`, not only at 100% with one
   font. Move the window between displays with different scale factors and
   confirm the pointer contract survives the geometry change.
10. **Two embedded views**: with two terminal panes in one window, mouse events,
    select-all and font actions must go only to the focused pane's session.
    Repeat the HiDPI and resize cases with both panes present.

## Kitty graphics (both platforms)

- Windows: run the automated `conpty_apc_survives_to_ghostty_graphics` and
  `wsl_client_apc_survives_wsl_pty_and_conpty` tests. They prove a raw APC and a
  Linux-client APC survive the bundled `conpty.dll` and the WSL -> ConPTY path.
  Removing `conpty.dll` must log the transport as Kitty-graphics-degraded; do
  not claim graphics support in that configuration.
- Linux/WSLg with `kitten` installed: inside pain run
  `kitten icat --transfer-mode=stream --detect-support=no --unicode-placeholder image.png`
  and `kitten icat --loop=-1 anim.gif`. The placeholder image must appear on its
  cursor cells (not "does not support reporting screen sizes in pixels"), and
  the GIF must animate. `tests/pty.rs` automates the placeholder case; keep the
  visual animation check manual.

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
- WSLg draws an extra native `− □ ✕` titlebar around the window. This is a
  display-server artifact: WSLg's Weston/Xwayland layer adds server-side
  decoration when forwarding the Wayland surface into a Windows window. Pain
  itself requests client-side decorations (`window_decorations: Client` in
  Silvestris's `window_options`), so on a desktop that honours that request the
  only chrome is pain's own Silvestris title band (as on native Windows). A
  compositor that forces server-side decorations can still add a frame; the
  doubled Windows-style chrome is specific to WSLg.

## Repair-pass interactive smoke evidence (2026-11-26)

Both platforms were exercised with a GUI binary built from the repair-pass
source. Automated input was delivered to the foreground window; screenshots
were captured per step into the local temp folder.

### Windows (native, GNU host, `target\debug\pain.exe`, `--features shell`)

Built with the lld recipe plus a GCC spec override that drops MinGW's
`default-manifest.o` (GPUI already embeds the Common-Controls manifest; without
the override lld reports a duplicate `MANIFEST` resource — see
[SOURCE-MAP.md](SOURCE-MAP.md)). Launching opens one titled window whose title
band follows the child's OSC title (`pwsh.exe` path). Evidence sequence:

1. `ui-win-d1-initial.png` — window opens with Silvestris traffic lights, drawn
   title band and a live PowerShell prompt at the working directory.
2. `ui-win-d2-echo-output.png` — typed command reaches the child PTY and its
   output renders inside the terminal; the OSC title stays in the chrome band.
3. `ui-win-d3-font-larger.png` / `ui-win-d4-font-reset.png` — `Ctrl+=` twice and
   `Ctrl+0` are consumed as app font actions (no escape bytes reach the shell).
4. `ui-win-d5-horizontal-resize.png` — width-only resize 1056 -> 1296 px with
   height pinned at 668 px; content reflows to the new column count and the
   scrollbar thumb follows. The PowerShell prompt stays usable.
5. `ui-win-d7-scrollbar-drag.png` — a left drag on the scrollbar thumb released
   inside the terminal terminates cleanly; a subsequent chrome-region click
   still reaches the window (it remained open), confirming drag ownership does
   not strand the thumb or swallow chrome events.

### Linux / WSLg (Fedora44, ext4 working copy, `--features shell`)

WSLg forwards the Wayland window to the Windows desktop as
`pain (Fedora44)`, so input/screenshots were driven from the Windows side while
the process ran in WSL. The app needs software rendering under WSLg
(`LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=gl MESA_LOADER_DRIVER_OVERRIDE=llvmpipe`);
otherwise the DirectX/Vulkan backend exits during startup (`Dropped Escape call`).
Evidence sequence:

1. `wslg-02-echo.png` — the window renders a styled TUI (multi-column status
   lines with a scrollbar thumb) and stays responsive to input.
2. `wslg-03-font-larger.png` / `wslg-04-font-reset.png` — font shortcuts applied.
3. `wslg-05-horizontal-resize.png` — width-only resize 1040 -> 1240 px, height
   pinned at 660 px; content reflows and the window stays open.
4. `wslg-06-scrollbar-drag.png` — thumb drag released inside the terminal; the
   window remained open (`still_open=True`). The WSLg window was partly occluded
   by another desktop window during capture, but the pain window and its
   scrollbar are visible in every shot.

### Windows release binary (`target\release\pain.exe`)

Once the Windows SDK is installed, set `GPUI_FXC_PATH` to its `x64\fxc.exe`
and build with the lld + manifest-spec recipe in [SOURCE-MAP.md](SOURCE-MAP.md).
`win-release-01-initial.png` through `win-release-04-horizontal-resize.png`
confirm the optimized binary opens with the OSC title in the Silvestris band,
receives typed input, applies `Ctrl+=` font actions and reflows on a
width-only 1056 -> 1276 px resize with height unchanged.

The headless M6 counters for both platforms are in
[docs/PERFORMANCE.md](PERFORMANCE.md); the Windows GUI build recipe and the
WSLg software-rendering requirement are in [SOURCE-MAP.md](SOURCE-MAP.md).

## Recording a result

Copy the `M6 ...` lines printed by `scripts/smoke.ps1` / `smoke.sh` into
[docs/PERFORMANCE.md](PERFORMANCE.md) when changing hot paths, and note the
platform, profile, terminal size and workload as described there.
