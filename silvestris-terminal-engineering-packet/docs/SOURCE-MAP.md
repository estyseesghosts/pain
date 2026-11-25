# Source Map and Revision Pins

This packet is intentionally source-grounded. The URLs below are immutable commit links so later upstream changes do not silently change the evidence behind an engineering decision. Before implementing the project, verify whether newer released versions materially change any API, then update the pin **deliberately** rather than replacing links with `main`.

## Research baseline — 2026-11-24

| Project | Branch/context | Revision |
|---|---|---|
| Ghostty | `main` research snapshot | `64a6fedfd26d1fa687733f9f411db5eb8e205ebe` |
| Wintty | `windows` branch research snapshot | `047cd33cd1a24602cc4887ac260e2f922943cac0` |
| Silvestris | `devel/sil-cocoa-fixture` | `0d5cc609171992d003994c8d8fd44b014b4d0b94` |
| Zed/GPUI | exact Silvestris GPUI pin | `6b720973027999bae402df23d469026f4cced988` |

The most important dependency rule is that the terminal application **must not choose its own GPUI revision**. Current Silvestris documentation says consumers must use the same GPUI revision and warns that two GPUI revisions yield incompatible types. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md) and [`Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml).

Ghostty is different: the research pin is upstream `main`, but the real implementation should use a separately released `vt-X.Y.Z` tag. Ghostty says `libghostty-vt` functionality is stable while API signatures remain in flux and explains that VT library releases are separately tagged. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).

## Ghostty source map

### Architecture/status

- [README.md @ `64a6fedf`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md)
  - `libghostty` / `libghostty-vt` status.
  - Cross-platform embeddable library intent.
  - API signatures still in flux.
  - `vt-X.Y.Z` release model.
  - Full Ghostty performance architecture background.

### Terminal state and effects

- [`include/ghostty/vt/terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h)
  - `GhosttyTerminal` ownership contract.
  - Explicit statement that drawing and PTY are embedder responsibilities.
  - VT write.
  - resize and cell pixel dimensions.
  - terminal options/effects/callbacks.
  - scrollback limits/memory.
  - viewport scrolling/scrollbar.
  - render hold/synchronized output.
  - selection/grid queries and terminal state.
  - thread-safety notes around compression/operations.

This is the primary source for deciding what belongs in `TerminalSession` versus the PTY/UI.

### Render bridge

- [`include/ghostty/vt/render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h)
  - render-state creation.
  - full/partial/clean dirty states.
  - two-phase begin/end update.
  - clean semantics.
  - dirty-row iterator.
  - row selection.
  - cell grapheme/style/resolved color data.
  - multi-get APIs.
  - cursor/palette/global render state.

- [`example/c-vt-render/src/main.c`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c)
  - lifecycle/reference for render state and iterators.
- [`example/c-vt-stream/src/main.c`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-stream/src/main.c)
  - incremental streaming/render-state reuse.

These are the primary sources for the row-cache and lock-handoff design.

### Keyboard

- [`include/ghostty/vt/key/encoder.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
  - cursor-key application;
  - keypad mode;
  - Alt-escape;
  - modifyOtherKeys;
  - Kitty keyboard flags;
  - backarrow;
  - `ghostty_key_encoder_set_from_terminal`.

- [`example/c-vt-encode-key/src/main.c`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-key/src/main.c)
  - event → encoder → byte sequence.

Primary rule derived from this source: do not maintain a parallel terminal-mode table in Rust.

### Mouse

- [`include/ghostty/vt/mouse/encoder.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/mouse/encoder.h)
- [`example/c-vt-encode-mouse/src/main.c`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c)

Use for terminal mouse reporting; common UI still decides local selection versus report-to-child.

### Paste

- [`include/ghostty/vt/paste.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/paste.h)
- [`example/c-vt-paste/src/main.c`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c)

Use for bracketed paste and unsafe-paste policy. Do not raw-write clipboard text.

### PTY implementation reference

- [`src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig)
  - POSIX `openpty`;
  - CLOEXEC;
  - `IUTF8`;
  - `TIOCSWINSZ`;
  - `setsid`;
  - `TIOCSCTTY`;
  - Windows/Posix split in Ghostty's own runtime.

We are not importing this module. It is implementation evidence/checklist for our platform code.

## Wintty source map

Wintty is a **Windows reference only**, not a dependency.

- [README](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/README.md)
  - Windows Ghostty-derived application context.

- [`src/pty.zig` @ `047cd33c`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig)
  - ConPTY is current Windows transport.
  - `CreatePseudoConsole`.
  - `ResizePseudoConsole`.
  - `ClosePseudoConsole`.
  - pipe strategy.
  - optional adjacent `conpty.dll`.
  - fallback behavior and graphics-protocol caveat.

- [`example/c-win32-terminal/src/main.c`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c)
  - minimal full-libghostty Win32 host.
  - wakeup callback/message-loop handoff.
  - key/text/surrogate handling checklist.
  - mouse/wheel/focus/DPI events.
  - resize-loop timer in a raw Win32 host.
  - example of **architecture we are intentionally not using** for rendering: Ghostty owns DX12 surface there.

## Silvestris source map

All Silvestris links are from `devel/sil-cocoa-fixture` at `0d5cc609171992d003994c8d8fd44b014b4d0b94`.

- [`Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml)
  - Rust/workspace version.
  - exact GPUI pin `6b720973027999bae402df23d469026f4cced988`.
  - release optimization profile.

- [`README.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/README.md)
  - Silvestris role as GPUI application shell/toolkit.
  - current platform maturity.
  - consumer entry points.

- [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md)
  - ownership table.
  - domain-view rule.
  - exact GPUI pin requirement.
  - startup.
  - theme/font guidance.
  - testing limits.

- [`crates/silvestris/src/lib.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/lib.rs)
  - `silvestris::init`.
  - font/theme/text/modal initialization.

- [`crates/silvestris/src/chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs)
  - window options.
  - `open_window`.
  - `WindowFrame`.
  - Windows native hit-test/control behavior.
  - Linux client-side decorations.
  - macOS note.

- [`crates/silvestris/src/shell.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/shell.rs)
  - measured content via GPUI allocation.
  - useful precedent for bounds-driven domain layout.

- [`crates/silvestris/src/responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs)
  - focus-aware responder chain.
  - `capture_keys` domain-canvas policy.

- [`crates/silvestris/src/native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs)
  - native platform frame boundary.
  - caret blink.
  - Windows unsafe/native code centralization.

- [`crates/silvestris/src/controls/scrollbar.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/controls/scrollbar.rs)
  - Aqua scrollbar currently driven by GPUI `ScrollHandle`.
  - reason terminal history cannot simply use it as source of truth.

- [`crates/silvestris/src/fonts.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/fonts.rs)
  - font registration.
- [`crates/silvestris/src/theme.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/theme.rs)
  - active theme/global state.

## GPUI source map

All GPUI/Zed links are at the **Silvestris-pinned** commit `6b720973027999bae402df23d469026f4cced988`.

- [`crates/gpui/src/element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs)
  - custom `Element`.
  - layout/prepaint/paint lifecycle.
  - state carried between phases.

- [`crates/gpui/src/text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs)
  - `TextRun`.
  - `shape_line`.
  - terminal text-shaping basis.

- [`crates/gpui/src/platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)
  - platform abstraction.
  - `InputHandler`/native text protocol.
  - window visibility/graphical environment context.

- [`crates/gpui/src/app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs)
  - `FocusHandle` and application context.
- [`crates/gpui/src/window.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/window.rs)
  - frame invalidation/window paint integration.

### Zed terminal as GPUI integration reference

These are **not dependencies**; they are examples at the exact pinned GPUI API:

- [`crates/terminal_view/src/terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs)
  - custom terminal element.
  - `BatchedTextRun`.
  - GPUI shaping/painting precedent.

- [`crates/terminal/src/terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs)
  - wakeup/event handling.
  - terminal synchronization.
  - resize suppression and regression test.

Do not copy Alacritty-specific terminal models into the Ghostty design.

## Updating this packet

When moving a source pin:

1. record old/new commit/tag;
2. inspect the exact files above;
3. note API/behavior differences;
4. update this map;
5. update affected architecture docs;
6. run headless terminal tests;
7. run Windows/Linux interactive smoke tests;
8. run performance counters/stress suite.

A GPUI update is coupled to Silvestris. A Ghostty VT update is coupled to the FFI wrapper and terminal behavior. Wintty is never a required pin unless code is actually copied/adapted under compatible licensing; normally it remains reference-only.

## Source integrity rules

- Prefer immutable commit/tag links in engineering docs.
- Do not cite `main` when an exact revision was inspected.
- Do not infer behavior from README when source/header documents it more precisely.
- Do not claim unsupported platform behavior based on compile success.
- Do not copy a source implementation blindly when its surrounding runtime assumptions differ.
- Treat comments in Wintty/Ghostty source as implementation evidence, but verify against actual API behavior during the compile spike.

## Deliverables

- Exact source/version table kept current.
- Stable links for every architecture claim.
- Deliberate Ghostty VT release selection.
- GPUI pin always inherited from Silvestris.
- Wintty clearly reference-only.
- Upgrade checklist.

## Avoidables

- Floating Git dependencies.
- mixed Ghostty header/library versions.
- Zed main links for pinned GPUI behavior.
- using Wintty's branch as a hidden dependency.
- unreviewed GPUI pin override.
- source citations without commit hashes.

## Regression traps

- upstream API changes make old FFI declarations compile against wrong runtime.
- Silvestris updates GPUI but terminal retains old direct dependency.
- Ghostty `main` changes render structs while package uses old prebuilt DLL.
- source docs silently point to moving branch and historical rationale becomes unreproducible.
- Wintty behavior is assumed to equal OS ConPTY even when its bundled `conpty.dll` path differs.

## Cross-source anchors required by this packet

- Ghostty architecture: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h)
- Silvestris ownership/pinning: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md)
- GPUI custom rendering: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs)
