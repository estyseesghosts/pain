# Silvestris Terminal Demo — Engineering Packet

This packet defines a deliberately small terminal application built as a **Silvestris consumer** on **Windows and Linux**. The program is intended to prove that a real terminal can live cleanly inside the current Silvestris/GPUI stack without importing the application complexity of Ghostty, Wintty, Zed, Windows Terminal, or a modern multi-pane terminal. The first useful version has one window, one terminal session, one shell, scrollback, keyboard and text input, selection/copy/paste, resize, focus, basic mouse support, and correct terminal colors. It does not have tabs, splits, profiles, a command palette, shell integration, session restoration, remote-control APIs, image protocols, shader systems, or a settings application.

The architectural decision is simple: **Ghostty owns terminal semantics; the application owns PTY transport; GPUI owns rendering and platform input; Silvestris owns the window shell and application chrome.** Current `libghostty-vt` is a particularly good fit because its terminal API explicitly states that a `GhosttyTerminal` keeps the screen, scrollback, cursor, styles, and modes while **not drawing and not talking to a PTY**; both are left to the embedder. Its render API exposes dirty/full/partial state and dirty-row iteration, so the GPUI renderer can redraw only what changed instead of copying or rebuilding the entire terminal every frame. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h) and [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Silvestris's current consumer contract already puts this terminal on the application side of the boundary. `docs/consumers.md` says Silvestris owns generic controls, window/chrome, themes, command routing and focus, while applications own domain models, sessions, and app-specific input policy; it explicitly calls emulator screens, previews, and code editors domain views. A terminal surface belongs in the same category. The app should therefore consume Silvestris public APIs, use `silvestris::init`, `chrome::window_options`/`open_window`, and place a custom terminal view inside `WindowFrame::child`; it should **not** turn terminal emulation into a generic Silvestris control. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`lib.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/lib.rs), and [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

GPUI is the rendering substrate. At the exact GPUI revision pinned by Silvestris, `gpui::Element` provides the `request_layout` → `prepaint` → `paint` lifecycle intended for custom elements. That is the right level for a terminal grid. We render the terminal as one custom element, batch adjacent equal-style cells into text runs, draw backgrounds/selection/cursor as quads, and shape text through GPUI's text system. We do **not** create one GPUI child per terminal cell. Zed's terminal at the same pinned commit is a useful precedent: its `TerminalElement` has a `BatchedTextRun` specifically for adjacent same-style cells, and its terminal backend avoids PTY resize spam when pixel changes do not change terminal geometry. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [GPUI text system](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs), and [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

## Pinned research baseline

These are **research/source pins for this packet**, not an instruction to track upstream `main` automatically in production.

| Project | Revision used by this packet | Why |
|---|---|---|
| Ghostty | `64a6fedfd26d1fa687733f9f411db5eb8e205ebe` | Current `main` inspected for `libghostty-vt`, render state, terminal, input and POSIX PTY APIs. |
| Wintty | `047cd33cd1a24602cc4887ac260e2f922943cac0` | Current `windows` branch inspected for ConPTY choices and the minimal Win32 host. |
| Silvestris | `0d5cc609171992d003994c8d8fd44b014b4d0b94` | Current `devel/sil-cocoa-fixture` branch requested for this design. |
| GPUI / Zed | `6b720973027999bae402df23d469026f4cced988` | **Exact revision already pinned by Silvestris.** The terminal app must use this same GPUI revision. |

Ghostty itself says `libghostty-vt` is already usable and its functionality is stable, but its API signatures are still in flux and releases are separately tagged as `vt-X.Y.Z`. The implementation should therefore select and lock a tested `vt-X.Y.Z` release after the initial compile spike rather than silently consuming Ghostty `main`. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md). Silvestris likewise requires consumers to use the same GPUI revision because two GPUI revisions create incompatible types. See [Silvestris `docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md) and [Silvestris `Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml).

## Intended runtime shape

```text
Silvestris / GPUI process
│
├─ WindowFrame
│   └─ TerminalView                         application/domain view
│       └─ TerminalElement                  one custom gpui::Element
│           ├─ cached row render data
│           ├─ batched shaped text
│           ├─ cell/background quads
│           ├─ selection quads
│           └─ cursor
│
└─ TerminalSession
    ├─ serialized GhosttyTerminal
    ├─ GhosttyRenderState
    ├─ GhosttyKeyEncoder
    ├─ GhosttyMouseEncoder                 introduced after keyboard baseline
    ├─ bounded/coalesced UI wakeup state
    └─ PlatformPty
        ├─ Windows: OS ConPTY
        └─ Linux: POSIX PTY
```

The initial implementation should be one Rust application crate plus the smallest FFI/build boundary needed for `libghostty-vt`. Do not split the project into a framework of reusable terminal crates before the first real terminal works. If raw bindings need isolation, keep them in a narrow `ghostty_vt`/`ffi` module or one `-sys` crate; all `unsafe` C interaction should terminate there. The rest of the program should manipulate safe Rust wrappers.

## What “minimal” means

The first supported terminal should be able to run an interactive shell and ordinary TUIs correctly. A useful acceptance set is PowerShell/cmd on Windows, a normal `$SHELL` on Linux, plus applications such as `vim`, `htop`/`btop`, and a terminal agent/CLI. Correctness includes alternate screen, cursor modes, ANSI/256/true color, resizing, Ctrl combinations, arrow/function keys, bracketed paste, selection and scrollback. This does **not** imply support for every protocol Ghostty itself supports. Kitty graphics, Sixel, terminal notifications, semantic prompt navigation, shell integration, drag/drop payloads, hyperlinks, and OSC clipboard operations can remain disabled or ignored until deliberately scoped.

The visual target is likewise modest: a single Snow Leopard-era Terminal.app-like Aqua window, using Silvestris chrome and a clean terminal canvas. Terminal content should not be “Aqua themed” after the fact. VT colors and the application's own ANSI palette belong to terminal semantics. Silvestris should provide the surrounding window, default mono font choice and chrome; Ghostty's parsed terminal state remains authoritative for per-cell color/style.

## How we keep it fast

Performance comes primarily from avoiding unnecessary architecture. PTY I/O never waits on the UI thread. Incoming bytes are read in chunks and fed to Ghostty under a short serialization lock. A single pending-wakeup flag coalesces output storms into GUI refreshes instead of scheduling one UI task per byte or read. During rendering, use Ghostty's two-phase render-state update: hold the terminal lock only for `ghostty_render_state_begin_update`, release it, finish with `ghostty_render_state_end_update`, and then iterate only dirty rows. This two-phase API exists specifically to minimize terminal lock hold time. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

On the GPUI side, unchanged rows are not reshaped. Dirty rows are converted into a small list of background quads plus adjacent styled text runs. The app should reuse render-state/row-iterator/cell handles across frames, matching Ghostty's own C examples, and clean dirty state only after a complete successful frame. See [Ghostty render example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c) and [stream example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-stream/src/main.c). Resize events are also coalesced: update terminal/PTY geometry only when rows/columns or relevant cell metrics actually change, following the same principle Zed uses in its pinned terminal implementation. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

## What we deliberately do not do

Do not embed Ghostty's Metal/OpenGL renderer or Wintty's DX12 renderer inside GPUI for this version. Wintty's minimal Win32 example proves the full `libghostty` runtime can own a DX12 surface and ConPTY when handed an `HWND`, but that is a different ownership model: Ghostty owns the terminal renderer there. Our app already has a GPUI window and scene, so a second renderer introduces shared-surface/composition and input ownership problems for no benefit to this demonstration. See [Wintty minimal Win32 host](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c).

Do not copy Ghostty's whole application architecture. Ghostty's README describes a dedicated read thread, write thread and render thread per terminal in the full application, but this app does not need a separate GPU render thread because GPUI already owns frame rendering. A dedicated reader and writer path is enough; the terminal renderer runs as part of GPUI's frame. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).

Do not adopt Wintty's WinUI 3 shell. Wintty is a reference for Windows PTY and edge cases, not a dependency. Its current Windows PTY implementation uses ConPTY and optionally a bundled newer `conpty.dll`; the OS fallback loses some graphics-protocol pass-through. Because graphics protocols are deliberately out of scope here, the first build should use the OS ConPTY and avoid shipping a private ConPTY DLL. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig).

## Document map

- [AGENTS.md](AGENTS.md) — non-negotiable implementation rules for coding agents.
- [ROADMAP.md](ROADMAP.md) — ordered milestones and acceptance gates.
- [docs/TERMINAL.md](docs/TERMINAL.md) — complete runtime architecture and state flow.
- [docs/GHOSTTY.md](docs/GHOSTTY.md) — `libghostty-vt` integration and FFI boundary.
- [docs/SILVESTRIS.md](docs/SILVESTRIS.md) — application/chrome/theme/responder integration.
- [docs/GPUI.md](docs/GPUI.md) — custom element, text shaping, focus and IME.
- [docs/WINDOWS-BACKEND.md](docs/WINDOWS-BACKEND.md) — ConPTY/process lifecycle.
- [docs/LINUX-BACKEND.md](docs/LINUX-BACKEND.md) — POSIX PTY/process lifecycle.
- [docs/OSX-BACKEND.md](docs/OSX-BACKEND.md) — deferred macOS design; not an active target.
- [docs/PERFORMANCE.md](docs/PERFORMANCE.md) — hot paths, budgets, counters and stress tests.
- [docs/SOURCE-MAP.md](docs/SOURCE-MAP.md) — exact upstream pins and source-file index.

## Package-wide deliverables

1. A Windows build that opens one Silvestris window, starts one ConPTY shell, renders through GPUI, handles keyboard/text, selection/copy/paste, scrollback and resize, and closes cleanly.
2. A Linux build with the same application/domain code and a POSIX PTY backend.
3. A single terminal renderer shared across both platforms.
4. A small Ghostty C-API boundary with no Ghostty GUI/runtime or renderer dependency.
5. Performance counters and stress tests sufficient to detect wakeup storms, lock contention, full-grid redraws, shaping churn and resize storms.
6. Documentation and tests that keep macOS explicitly deferred until hardware/CI exists.

## Package-wide avoidables

- No tab/split/session model.
- No terminal “framework” abstraction beyond the platform PTY seam and narrow Ghostty wrapper.
- No per-cell GPUI element tree.
- No raw PTY writes for paste when Ghostty can apply bracketed-paste semantics.
- No independent GPUI upgrade from the Silvestris pin.
- No tracking Ghostty `main` in a shipping build.
- No bundled ConPTY DLL in the first Windows version.
- No image protocols in the first version.
- No macOS implementation code claimed as supported without a test machine/CI.
- No blocking PTY read/write, process wait or large terminal parse operation on GPUI's foreground thread.

## Package-wide regression traps

The biggest regressions will not look like ordinary bugs. They will look like “small convenience changes”: sending a repaint for every read, reshaping every row every frame, writing terminal replies synchronously while the terminal lock is held, passing pixel resize changes through ConPTY on every mouse move, letting Silvestris global shortcuts steal terminal keystrokes, treating a generic GPUI `ScrollHandle` as the terminal's scrollback source of truth, or setting `TERM` to a capability set the renderer does not actually implement. The roadmap treats these as explicit gates rather than cleanup work.

## Source anchors

- Ghostty: [README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)
