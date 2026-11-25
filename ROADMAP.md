# Roadmap

This roadmap gets from “Ghostty terminal state rendered inside a Silvestris window” to a small, usable Windows/Linux terminal without turning the demo into a general terminal platform. Milestones are ordered so each layer is proven before the next one depends on it. A milestone is complete only when its acceptance checks pass; do not pull later features forward to make an earlier demo look more complete.

The design follows three upstream facts. First, `libghostty-vt` deliberately leaves both drawing and PTY transport to the embedder while owning screen, scrollback, cursor, style and mode state. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Second, Silvestris says application/domain views own app-specific state and input policy while Silvestris owns generic chrome/focus/commands. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md). Third, GPUI custom elements are explicitly built around `request_layout`, `prepaint` and `paint`, which gives us a direct low-overhead place to render the grid. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

## M0 — Pin dependencies and prove `libghostty-vt` from Rust

### Goal

Prove that the chosen Ghostty VT release can be built, linked and safely called from the same Rust/GPUI toolchain as the current Silvestris branch before any platform PTY work begins.

### Deliverables

- Pin Silvestris to the chosen `devel/sil-cocoa-fixture` commit and use exactly its GPUI revision.
- Select a released `vt-X.Y.Z` tag close to the researched Ghostty source. The packet's research revision is `64a6fedfd26d1fa687733f9f411db5eb8e205ebe`, but production code must not silently track `main`.
- Build/link `libghostty-vt` on Windows and Linux.
- Add the narrowest raw bindings needed for:
  - terminal create/free;
  - VT write;
  - resize;
  - render-state create/update/iterate/clean;
  - key encoder;
  - paste;
  - terminal data/options used by MVP effects.
- Add a safe Rust wrapper whose Drop implementation releases every owned Ghostty handle exactly once.
- Add a headless unit/integration test: create an `80×24` terminal, feed styled VT bytes, update render state, and assert visible cell text/style/cursor data.
- Record the chosen Ghostty VT tag and build commands in [docs/SOURCE-MAP.md](docs/SOURCE-MAP.md).

Ghostty's own C examples create terminal/render-state/iterator handles once and reuse them, so the wrapper should expose long-lived objects rather than allocate FFI handles per render. See [render example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c) and [stream example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-stream/src/main.c).

### Avoidables

- No PTY yet.
- No GPUI terminal renderer yet.
- No `bindgen` at runtime/build time if checked-in minimal declarations are easier to audit; either approach is acceptable, but avoid importing the entire Ghostty C surface.
- No full `libghostty` runtime.
- No Wintty dependency.
- No `unsafe` outside the FFI layer.

### Regression traps

- ABI mismatch from headers and library built from different Ghostty revisions.
- Assuming Ghostty C API signatures are frozen because behavior is stable. Ghostty explicitly says signatures are still in flux. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).
- Accidentally pulling another GPUI version through an independent dependency. Silvestris warns this produces incompatible types. See [Silvestris consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

### Acceptance gate

A Windows and Linux CI/local test can create, feed and inspect a Ghostty terminal without opening a window and without leaking handles under a basic repeated-create/free loop.

---

## M1 — Static GPUI terminal element

### Goal

Render a Ghostty terminal snapshot in a Silvestris window without a live shell.

### Deliverables

- Start a minimal Silvestris application:
  - `silvestris::init`;
  - `chrome::window_options`;
  - `chrome::open_window`;
  - `WindowFrame::child`.
- Add `TerminalView` and one custom `TerminalElement`.
- Measure the resolved mono font and derive cell width/line height from actual GPUI font metrics.
- Feed canned VT fixtures into `GhosttyTerminal`.
- During prepaint:
  - obtain terminal bounds;
  - calculate columns/rows;
  - update Ghostty render state;
  - iterate dirty rows;
  - resolve styles/colors;
  - batch adjacent compatible cells into row text runs.
- During paint:
  - draw background quads;
  - draw selection quads if fixture includes selection;
  - shape/paint batched text;
  - draw cursor.
- Add snapshot/logical tests for row batching and cell-to-pixel geometry.

The renderer should follow the same performance principle as Zed's pinned `TerminalElement`, which batches adjacent same-style cells, while remaining independent of Zed's terminal crate. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs) and [GPUI text system](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

### Avoidables

- No cell widgets/divs.
- No live PTY.
- No generic GPUI scroll container around the grid.
- No font ligature feature work.
- No image protocol.
- No animations except cursor fixture if needed.

### Regression traps

- Shaping every cell independently.
- Assuming one Unicode scalar equals one terminal cell; use Ghostty grapheme data.
- Treating empty cells as strings that must be shaped.
- Applying Silvestris text colors on top of terminal-provided ANSI colors.
- Calling `ghostty_render_state_clean` before the row cache is complete.

### Acceptance gate

Canned ANSI/256/true-color fixtures, wide characters, underlines/bold and cursor placement render correctly in one Silvestris window. Re-rendering an unchanged fixture does not rebuild every row.

---

## M2 — Shared terminal session and wakeup model

### Goal

Add the concurrency model before adding real platform PTYs.

### Deliverables

- `TerminalSession` owns/coordinates:
  - serialized `GhosttyTerminal`;
  - `GhosttyRenderState`;
  - key encoder;
  - generation counter;
  - pending-GPUI-wakeup flag;
  - writer queue interface.
- Implement the render-state two-phase update:
  - lock terminal;
  - `ghostty_render_state_begin_update`;
  - unlock;
  - `ghostty_render_state_end_update`.
- Implement a coalesced wakeup path usable by a synthetic byte producer.
- Add instrumentation counters from [docs/PERFORMANCE.md](docs/PERFORMANCE.md).
- Stress with a background fixture producer pushing many chunks per second.

Ghostty documents that only the begin phase needs terminal access and the end phase exclusively touches render-state memory. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h). GPUI already has foreground/background executor concepts, but the terminal does not need a general async runtime to exploit them. See [GPUI platform/executor-facing APIs](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

### Avoidables

- No Tokio.
- No per-chunk `cx.notify` storm.
- No lock held through text shaping or paint.
- No unbounded growth of pending rendered snapshots.

### Regression traps

- Lost wakeup: producer writes after UI tests/clears pending state but before it exits.
- Multiple simultaneous render-state updates.
- Re-entering terminal parsing from a Ghostty effect callback.
- Holding the terminal lock while a channel send can block.

### Acceptance gate

Synthetic high-rate output keeps UI responsive, wakeups are substantially lower than input chunks, and terminal lock hold time is bounded to VT processing/render-state begin work rather than GPUI shaping.

---

## M3 — Windows ConPTY backend

### Goal

Make the Windows build a real one-session terminal using OS ConPTY.

### Deliverables

- Implement ConPTY creation with input/output pipes and `CreatePseudoConsole`.
- Spawn the child using the required pseudoconsole process attribute.
- Implement reader thread → Ghostty VT write.
- Implement writer thread/queue → ConPTY input.
- Implement process-exit observation and clean shutdown.
- Resize ConPTY only when computed grid dimensions change.
- Default shell policy documented and overrideable from CLI/config without a settings UI.
- Test `cmd.exe` and PowerShell; add WSL smoke test if available but do not make WSL a requirement.
- Handle Windows key events and committed Unicode correctly through GPUI, including IME/surrogate cases.
- Clipboard copy/paste works.
- Selection works with a TUI mouse-mode escape policy.

Wintty's current `src/pty.zig` confirms ConPTY is its sole Windows transport and shows `CreatePseudoConsole`/`ResizePseudoConsole`/`ClosePseudoConsole`, while its minimal Win32 host is a checklist for key, text, focus, DPI and resize edge cases. See [Wintty PTY](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig) and [Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c).

### Avoidables

- Do not host a Ghostty DX12 surface.
- Do not use WinUI 3.
- Do not bundle Wintty.
- Do not bundle a private `conpty.dll` in MVP.
- Do not adopt IOCP solely because Wintty uses it; one blocking reader and writer thread is acceptable for one terminal.

### Regression traps

- UI thread blocked by `ReadFile`, `WriteFile`, process wait or pipe connection.
- Inheritable handle leaks into child process.
- Sending stale resize dimensions during drag.
- Keydown generating duplicate text when IME/character input also commits text.
- ConPTY close order hanging the reader thread.
- Ctrl+C killing the GUI process instead of being sent to the child terminal.

### Acceptance gate

One Windows window can run an interactive shell for an extended session, run `vim`, accept paste, resize repeatedly and exit without hangs, runaway CPU or leaked child processes.

---

## M4 — Linux POSIX PTY backend

### Goal

Use the same terminal/session/renderer with a Linux PTY.

### Deliverables

- `openpty` master/slave allocation.
- master `FD_CLOEXEC`;
- UTF-8 terminal mode where supported;
- child session/process group;
- controlling terminal via `TIOCSCTTY`;
- child stdio attached to slave;
- unused FDs closed on both sides;
- `$SHELL` launch with `/bin/sh` fallback;
- `TIOCSWINSZ` on grid resize;
- child exit/reaping;
- shared reader/writer session flow;
- Wayland and X11 smoke testing through GPUI/Silvestris, with no terminal-specific compositor code.
- conservative `TERM`/`COLORTERM` policy.

Ghostty's current POSIX PTY implementation is a concrete checklist: `openpty`, CLOEXEC, `IUTF8`, `TIOCSWINSZ`, `setsid`, `TIOCSCTTY`. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig). Silvestris already makes Linux window chrome/platform behavior its concern. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

### Avoidables

- No GTK.
- No direct Wayland/X11 code.
- No shell integration installation.
- No utmp/wtmp integration in MVP.
- No login-shell policy unless explicitly needed.
- No `TERM=xterm-ghostty` yet.

### Regression traps

- Parent retaining slave FD, preventing EOF.
- Child retaining master FD.
- Missing controlling TTY causing job control failures.
- Environment changes that break user's locale.
- Failing to reap child.
- SIGWINCH resize mismatch.
- Generic app shortcuts stealing terminal Ctrl sequences.

### Acceptance gate

The Linux build runs the user's shell, `vim` and a mouse TUI on at least one Wayland and/or X11 setup available to development, resizes correctly and closes without orphaning the child.

---

## M5 — Correct input, selection, clipboard and scrollback

### Goal

Move from “shell appears” to a terminal that is comfortable enough for daily CLI work.

### Deliverables

- Key encoder synchronized from current Ghostty terminal modes for every relevant key event.
- GPUI input-handler integration for text/IME.
- Bracketed paste via Ghostty terminal paste API.
- Local selection:
  - character drag;
  - word selection;
  - line selection if Ghostty selection gesture API makes it cheap;
  - copy command.
- Scroll wheel:
  - local scrollback when terminal mouse reporting is not active or escape modifier is held;
  - terminal mouse encoding when reporting is active.
- Terminal viewport remains Ghostty-owned.
- Optional Aqua scrollbar is a view/controller over Ghostty scrollbar values, not a second scroll model.
- Focus policy integrated with Silvestris responder chain.

Ghostty provides key encoder synchronization from terminal state, mouse encoding and terminal-aware paste APIs. See [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h), [mouse example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c), and [paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c). Silvestris already has responder capture designed for domain canvases. See [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs).

### Avoidables

- No hand-coded table for terminal key escape sequences.
- No raw clipboard text write to PTY.
- No GPUI `ScrollHandle` as terminal source of truth.
- No hyperlink UI yet.
- No OSC clipboard program access unless deliberately added with security policy.

### Regression traps

- Shift/Ctrl modifiers being consumed by app command routing before terminal sees them.
- Selection coordinates drifting after scroll/resize.
- Copying visual wrapped lines incorrectly.
- Paste newline injection outside bracketed paste.
- TUI mouse mode making local selection impossible.

### Acceptance gate

Keyboard-heavy apps, mouse TUIs and clipboard workflows work predictably on Windows and Linux, including after long scrollback and multiple resizes.

---

## M6 — Performance hardening

### Goal

Make the minimal architecture demonstrably lean under output and resize stress.

### Deliverables

- Counters for bytes/chunks, wakeups, render-state updates, dirty rows, row rebuilds, text runs shaped, lock hold time, writer queue depth and resize requests.
- Stress suite in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).
- No shaping for clean rows.
- No full visible-grid rebuild for cursor blink.
- PTY output burst does not schedule an unbounded number of UI callbacks.
- Resize storm coalescing.
- Bounded scrollback configured deliberately.
- Optional idle scrollback compression only if profiling shows value and the implementation can schedule it without foreground stalls.

Ghostty's render state distinguishes clean/partial/full updates and has dirty-row iteration. See [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h). Zed's terminal resize guard is a direct regression reference. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

### Avoidables

- Micro-optimizing parser code owned by Ghostty.
- Adding a custom GPU renderer.
- SIMD work in app code before measuring.
- “Performance” changes that bypass terminal correctness or GPUI text shaping.

### Regression traps

- Optimized cache failing to invalidate on font/DPI/theme/terminal resize.
- Wakeup batching adding visible interactive input latency.
- Lock-free complexity causing dropped terminal state.
- Scrollback compression running synchronously under interactive load.

### Acceptance gate

The application remains interactive during large output, repeated resize, cursor blinking and long sessions; counters show the expected dirty-row and wakeup coalescing behavior rather than accidental full-frame work.

---

## M7 — Minimal release-quality shell

### Goal

Package a small program rather than an engineering demo, without broadening the terminal feature set.

### Deliverables

- Stable one-window launch/close behavior.
- App/window title follows terminal title effect with a safe fallback.
- Small CLI/config surface only for essentials such as shell command, font size and scrollback cap if required.
- Windows and Linux packaging instructions.
- Clear unsupported-feature list.
- Crash/error reporting to stderr/log file, not a settings/status subsystem.
- Version/source attribution for bundled `libghostty-vt`.
- Regression smoke script/checklist.
- README screenshots may be added, but visual polish must not block terminal correctness.

Silvestris already has window/chrome and theme entry points; do not replace them with terminal-specific windowing. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs) and [`lib.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/lib.rs). GPUI remains the only UI renderer. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

### Avoidables

- No tabs/splits “for release.”
- No graphical preferences window.
- No auto-update system.
- No plugin API.
- No macOS binary.
- No feature claims inherited from Ghostty that this renderer does not implement.

### Regression traps

- Packaging Ghostty headers/library at a different version than the bindings.
- Setting a terminal identity/terminfo richer than implemented features.
- Adding startup work that makes the demo feel like Wintty/WinUI rather than a small native app.
- Treating Windows-only behavior as the cross-platform default.

### Acceptance gate

Fresh installs on Windows and Linux can launch one shell quickly, run normal interactive CLI/TUI work, resize, copy/paste, close cleanly, and stay responsive during sustained output. The remaining issue list is explicitly “nice terminal features,” not fundamental transport/render/input correctness.

---

## Deferred — macOS

The platform abstraction must leave room for a POSIX/macOS PTY and macOS-specific key/text behavior, but no macOS code should be merged as “supported” while there is no test Mac. [docs/OSX-BACKEND.md](docs/OSX-BACKEND.md) documents the future seam.

## Source anchors

- Ghostty: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [POSIX PTY](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)

## Delivery status (M5–M7)

All three remaining milestones are delivered; the gates above are met by the
following evidence. Re-run the commands when changing hot paths.

- M5: viewport scrolling uses `ghostty_terminal_scroll_viewport` with a
  Ghostty-backed scrollbar view; wheel reports go to mouse-reporting TUIs,
  otherwise to local history. Word/line selection gestures and copy/paste are
  covered by `tests/headless.rs` and `src/terminal/selection.rs`; title effects
  are exercised in `src/terminal/session.rs`.
- M6: `tests/m6.rs` prints the counters recorded in
  [docs/PERFORMANCE.md](docs/PERFORMANCE.md). Flood, cursor-only and resize
  stress cases assert wakeup coalescing, zero clean-row shaping and duplicate
  resize suppression. Scrollback caps are set in `SessionConfig`.
- M7: release steps, CLI, log location and version attribution are in
  [docs/PACKAGING.md](docs/PACKAGING.md); unsupported features are listed in
  [docs/SUPPORTED.md](docs/SUPPORTED.md); the cross-platform checklist and
  scripts are in [docs/SMOKE.md](docs/SMOKE.md).

Run `scripts/smoke.ps1` on Windows and `scripts/smoke.sh` on Linux/WSLg. No
synthetic keystrokes or clicks are sent to the desktop by those scripts.
