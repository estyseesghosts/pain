# AGENTS.md

This repository is a deliberately minimal Windows/Linux terminal application built with Silvestris/GPUI and `libghostty-vt`. Agents must preserve the architectural boundary in this file. “Making it more like Ghostty/Wintty/Zed” is not a valid reason to expand scope: those projects are source references, not product requirements.

## Source baseline

Before modifying terminal architecture, inspect the pinned sources in [docs/SOURCE-MAP.md](docs/SOURCE-MAP.md). This packet was researched against Ghostty `64a6fedfd26d1fa687733f9f411db5eb8e205ebe`, Wintty `047cd33cd1a24602cc4887ac260e2f922943cac0`, Silvestris `devel/sil-cocoa-fixture` at `0d5cc609171992d003994c8d8fd44b014b4d0b94`, and the exact GPUI/Zed commit Silvestris pins, `6b720973027999bae402df23d469026f4cced988`. Silvestris explicitly requires consumers to share its GPUI revision because otherwise GPUI types are incompatible. Do not independently upgrade GPUI. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md) and [Silvestris workspace pin](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml).

For Ghostty, development research may use the current source pin, but a shipping implementation must pin a tested `libghostty-vt` release tag. Ghostty states that the VT functionality is stable but C API signatures remain in flux and releases are separate `vt-X.Y.Z` tags. Do not consume upstream `main` implicitly. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).

## Coding style

Rust code must follow **Google's Rust Style Guide** unless an established repository-local rule is more specific:

https://google.github.io/styleguide/rust/

Keep `unsafe` code small, documented and mechanically auditable. Raw C handles and Windows/POSIX handles should not leak into general UI modules. Every `unsafe` block must state the invariant being relied upon. FFI wrapper types must have clear ownership/drop behavior; do not represent owned Ghostty handles as freely copyable values.

## Non-negotiable ownership boundaries

**Ghostty owns terminal semantics.** Do not write a second ANSI/VT parser, terminal grid, scrollback engine, application-mode state machine, bracketed-paste implementation, or key-sequence table. A `GhosttyTerminal` already owns screen, scrollback, cursor, styles and modes while intentionally leaving drawing and PTY transport to the embedder. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Use the published render, key, mouse, paste and terminal APIs instead of reaching into Ghostty internals.

**The application owns PTY/process transport.** Windows uses OS ConPTY; Linux uses a POSIX PTY. The common terminal layer may request `write`, `resize`, `close`, and process state, but must not know `HPCON`, POSIX file descriptors, `STARTUPINFOEX`, `TIOCSWINSZ`, or child-pre-exec mechanics.

**GPUI owns drawing, focus, platform input and frame scheduling.** The terminal surface is one custom `gpui::Element`. The GPUI `Element` contract is `request_layout`, then `prepaint`, then `paint`; carry only the data required by the next phase. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs). Use GPUI text shaping and paint primitives rather than embedding Ghostty/Wintty GPU surfaces.

**Silvestris owns generic application UI.** Use `silvestris::init`, `chrome::window_options`/`open_window`, `WindowFrame`, themes, command/responder infrastructure and platform frame helpers. The terminal itself is an application/domain view. Silvestris explicitly distinguishes generic UI from application-owned domain views. See [Silvestris `docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

## Scope guard

The active target is one terminal session in one window on Windows and Linux. Do not add tabs, splits, detachable panes, profiles, workspaces, persistent sessions, command palette, plugin API, shell integration, remote control, SSH client, terminal multiplexing, embedded file browser, settings GUI, global hotkey/quick terminal, image protocol rendering, shader effects, ligature configuration matrix, or theme marketplace.

Do not implement macOS functionality. `docs/OSX-BACKEND.md` is a future design note so the platform abstraction does not become Windows/Linux-specific by accident. macOS is neither built nor tested until real hardware or reliable CI is available.

A feature is allowed in the MVP only when it is required for an ordinary interactive terminal: shell launch/exit, terminal output, keyboard/text, focus, resize, ANSI styles/colors, cursor, alternate screen, scrollback, selection/copy, paste, and enough mouse reporting for mainstream TUIs. Hyperlinks, notifications, semantic prompt metadata, OSC clipboard reads/writes and images are later work.

## Expected source shape

Keep the implementation shallow. A reasonable first tree is:

```text
src/
  main.rs
  app.rs
  terminal/
    mod.rs
    session.rs
    ghostty.rs
    input.rs
    render.rs
    selection.rs
  platform/
    mod.rs
    windows.rs
    linux.rs
```

If raw C declarations make `ghostty.rs` unwieldy, a small `ghostty-vt-sys` crate is acceptable. Do not split “core”, “runtime”, “renderer”, “protocol”, “model”, and “platform” into separate crates before the implementation proves those boundaries are useful.

## Threading and serialization rules

Never touch the same `GhosttyTerminal` concurrently. Current Ghostty documentation explicitly marks terminal operations such as scrollback compression as not thread-safe with writes, rendering, searches and other access. Serialize all terminal access through one lock or one strict owner. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

The PTY reader must not run on GPUI's foreground thread. Read chunks, acquire the terminal lock, call `ghostty_terminal_vt_write`, release the lock, and coalesce a GPUI wakeup. The terminal's `WRITE_PTY` effect must enqueue response bytes to the PTY writer rather than synchronously blocking on OS I/O while the terminal lock is held.

The render path should use `ghostty_render_state_begin_update` while the terminal lock is held, release the lock, then call `ghostty_render_state_end_update`. Ghostty added this split specifically so only the begin phase needs terminal synchronization. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h). A convenience single-call update is acceptable for the first rendering spike only; the production path should use the split update once PTY reading is concurrent.

Do not create one task or notification per byte. Use an atomic/coalesced “wake pending” state. When the UI consumes the wakeup it clears the flag after recording the latest generation. If more output arrives meanwhile, the writer/reader may schedule one more wakeup.

## Rendering rules

Render the entire terminal surface through one custom `TerminalElement`; never create an element per cell. GPUI's custom element API is intended for specialized layout/paint work. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

Use Ghostty's render state as the source of visible terminal state. Maintain row-level caches. On `DIRTY_PARTIAL`, iterate `ghostty_render_state_row_iterator_next_dirty`; on `DIRTY_FULL`, rebuild all visible rows. Query row-local selection once per row instead of a `selected` FFI lookup for every cell. Ghostty documents this specifically as the cheaper policy. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h) and [render example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c).

Batch adjacent cells into text runs when their effective font/style/foreground treatment is compatible. Zed's terminal at the pinned GPUI commit explicitly has `BatchedTextRun` for adjacent same-style cells; use that as evidence for the batching strategy, not as code to copy wholesale. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs). Shape through GPUI's `TextSystem::shape_line`; draw solid backgrounds, selection and cursor with GPUI paint primitives. See [GPUI `text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

Do not mark Ghostty render state clean until the entire frame's data has been consumed successfully. `ghostty_render_state_clean` clears global and per-row dirty flags and is documented for complete-frame success. A panic/error before completion must leave the next frame able to rebuild safely.

## Input rules

Keyboard encoding must follow the current terminal modes. Before encoding a terminal key, synchronize `GhosttyKeyEncoder` from `GhosttyTerminal` with `ghostty_key_encoder_set_from_terminal`; do not hand-maintain cursor-key application mode, keypad mode, modifyOtherKeys, backarrow mode, Alt prefix or Kitty keyboard flags. See [Ghostty key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h) and [key example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-key/src/main.c).

Text committed by IME is not the same thing as physical key press/release events. Use GPUI's input-handler path for composition/committed text; GPUI's platform `InputHandler` is the bridge for native text protocols and uses UTF-16 ranges unless otherwise stated. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs). Never derive committed Unicode solely from keycodes.

Paste must go through Ghostty's terminal-aware paste API, not a raw PTY write. The Ghostty paste example demonstrates bracketed paste and safe/unsafe handling. See [Ghostty paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c). Copy is local: obtain the current selection text from Ghostty and put it in the GPUI/platform clipboard.

Mouse reporting and local selection are separate modes. If the running TUI has mouse tracking enabled, send encoded terminal mouse events; preserve an explicit escape modifier/policy for local text selection. Do not make the UI “sometimes select and sometimes report” based on undocumented heuristics.

## Resize rules

Cell metrics are derived from the actual resolved GPUI monospace font and current scale, never from an assumed `8×16`. Compute rows/columns from the terminal element's allocated content bounds. Call both `ghostty_terminal_resize` and platform PTY resize when terminal grid geometry changes. Ghostty also accepts cell pixel dimensions because size-report and image-related protocols need them. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Do not send PTY resize events for fractional/pixel-only changes that leave the terminal grid unchanged. Zed's pinned terminal has this exact guard and a regression test for it. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs). During active window drag/resize, coalesce repeated equivalent terminal sizes.

## Silvestris integration rules

The terminal must use Silvestris public APIs and the same GPUI pin. Do not vendor or patch Silvestris inside the terminal app. Fix generic Silvestris bugs in Silvestris and move the consumer pin deliberately. See [Silvestris `docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

Use Silvestris `WindowFrame` for titlebar/chrome. Use the responder chain/focus gate so the terminal owns ordinary typing and terminal shortcuts while app-level commands are intentionally whitelisted. `ResponderEntry::capture_keys` is already designed for domain canvases that must swallow keys while allowing selected shell commands outward. See [Silvestris `responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs).

Do not make GPUI `ScrollHandle` authoritative for terminal history. Ghostty owns terminal scrollback and viewport. If an Aqua `Scrollbar` is added, bridge its thumb/track behavior to Ghostty's terminal scrollbar state and viewport API. See [Silvestris scrollbar](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/controls/scrollbar.rs) and [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## Platform rules

Windows starts with the OS-provided ConPTY only. Wintty currently supports an optional adjacent `conpty.dll` to preserve extra graphics protocols that the in-box host may strip; those graphics protocols are out of MVP scope, so do not import that distribution complexity. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig).

Linux uses POSIX PTY primitives. Ghostty's own current PTY code uses `openpty`, `FD_CLOEXEC`, `IUTF8`, `TIOCSWINSZ`, `setsid`, and `TIOCSCTTY`. Use it as a checklist for process/TTY mechanics, not as a module to vendor. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig).

Do not set `TERM=xterm-ghostty` for the MVP. We are not the full Ghostty application and will initially omit protocols Ghostty advertises. Prefer a conservative capability identity such as `xterm-256color` plus `COLORTERM=truecolor`, then revisit when the actual supported protocol matrix is known.

## Required tests before calling a milestone complete

Every milestone in [ROADMAP.md](ROADMAP.md) has its own acceptance checks. At minimum, agents must add tests for any fixed regression involving terminal parsing, resize coalescing, wakeup coalescing, selection coordinates, key mapping, UTF-8/IME input or process lifecycle. Platform code needs unit-testable helpers around raw OS calls so important behavior is not testable only by launching a GUI.

Interactive smoke tests must cover Windows and Linux separately. A Windows-only success does not imply the POSIX process/TTY path works, and a Linux-only success does not imply ConPTY key/text/resize behavior works.

## Deliverables for every change

- The smallest code change that closes the stated milestone/bug.
- Tests for new pure logic and every regression that can be reproduced without the full GUI.
- Source comments only where an invariant or platform quirk would otherwise be easy to break.
- Any source-pin/API change reflected in `docs/SOURCE-MAP.md`.
- Performance-sensitive changes accompanied by relevant counters or a before/after stress measurement from `docs/PERFORMANCE.md`.

## Avoidables

- New dependencies when `std`, current Silvestris/GPUI, the Windows crate, or the narrow Ghostty API already solves the problem.
- Async runtimes solely for one PTY.
- UI-thread file/process/PTY blocking.
- Copies of whole scrollback or whole render grids in the hot path.
- Per-cell allocations or FFI calls that can be row-batched.
- Unbounded channels for high-rate output without backpressure/coalescing policy.
- Hiding platform errors and continuing with partially initialized sessions.
- Making the renderer understand ANSI escape sequences.
- Making the PTY backend understand colors/cursor/terminal modes.
- Adding configuration options before a concrete need exists.

## Regression traps

- Lock inversion between terminal state and PTY writer.
- Calling a terminal effect callback that re-enters `ghostty_terminal_vt_write` on the same terminal.
- Clearing dirty state before the GPUI frame actually captures all required rows.
- Losing a wakeup when clearing the pending flag concurrently with PTY output.
- Treating `WM_KEYDOWN` text as equivalent to committed Unicode on Windows.
- Losing high/low surrogate or IME composition handling.
- Sending raw pasted newlines when bracketed paste is active.
- Reflow/scrollback corruption when resize, alternate-screen transitions and viewport scrolling interleave.
- Global Silvestris shortcuts stealing Ctrl/Alt sequences from terminal applications.
- `TERM` advertising capabilities that are disabled.
- Full-grid text shaping during cursor blink.
- Recreating Ghostty encoder/render iterator handles every frame.
- Accidental second GPUI revision through a transitive dependency.
- “Temporary” macOS code paths that are never tested but become treated as supported.

## Source anchors

- Ghostty: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs)
