# Ghostty Integration

The application uses **`libghostty-vt`**, not Ghostty's GUI application and not the full Ghostty GPU surface runtime. This distinction is the foundation of the project. Ghostty's current README describes `libghostty-vt` as the first separately released part of the embeddable library: it focuses on parsing terminal sequences and maintaining terminal state, is available across Windows/Linux/macOS/WebAssembly, and has stable functionality while API signatures remain in flux. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).

The public terminal header makes the ownership contract explicit: `GhosttyTerminal` is a complete terminal-emulator state object containing screen, scrollback, cursor, styles and modes, but **“it doesn't draw and it doesn't talk to a pty. Both of those are yours.”** See [`include/ghostty/vt/terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). That is why it fits a GPUI/Silvestris terminal more cleanly than embedding Ghostty's Metal/OpenGL renderer or Wintty's DX12 renderer.

Silvestris already expects application-owned domain views and app-specific session/input policy. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md). GPUI's custom-element and text APIs then become the renderer. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs) and [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

## 1. Versioning policy

Do not ship against Ghostty `main`. Research for this packet used commit `64a6fedfd26d1fa687733f9f411db5eb8e205ebe`, whose tree includes the current render-state, key, mouse, paste and terminal APIs. Production implementation should:

1. choose a released `vt-X.Y.Z` tag;
2. build the library and headers from that exact tag;
3. bind only those exact headers;
4. record the tag and resulting library identity in `docs/SOURCE-MAP.md`;
5. upgrade deliberately with an API diff and regression suite.

Ghostty explicitly documents separate `vt-X.Y.Z` releases and says signatures remain in flux. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md).

The Rust wrapper should expose a small internal version string/assertion if the C API provides it, so mismatched runtime DLL/static library and header bindings fail clearly instead of corrupting memory.

## 2. What we use

### Terminal

Use:

- `ghostty_terminal_new`;
- `ghostty_terminal_free`;
- `ghostty_terminal_vt_write`;
- `ghostty_terminal_resize`;
- `ghostty_terminal_get`;
- `ghostty_terminal_set`;
- `ghostty_terminal_scroll_viewport`;
- selection/grid-reference APIs;
- paste API;
- formatter/selection text API as needed for copy;
- optional memory usage/compression later.

The terminal is created with an initial nonzero row/column size. Feed PTY output directly to `ghostty_terminal_vt_write`. Do not preprocess escape sequences.

### Render state

Create once per terminal renderer and reuse:

- `ghostty_render_state_new/free`;
- `ghostty_render_state_begin_update`;
- `ghostty_render_state_end_update`;
- `ghostty_render_state_get`;
- row iterator;
- row cells;
- `ghostty_render_state_clean`.

The current header provides global dirty state (`FALSE`, `PARTIAL`, `FULL`), cursor/color bundles, dirty row iteration, row-local selection, UTF-8 grapheme data and resolved foreground/background color queries. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Ghostty's C render example demonstrates the intended lifecycle: create terminal/render state/iterators once, update, inspect dirty state, iterate dirty rows/cells, and clean after drawing. See [render example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c). The stream example reinforces reuse rather than recreation. See [stream example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-stream/src/main.c).

### Key encoder

Create one `GhosttyKeyEncoder` per session and reuse it. Before encoding a terminal key event, synchronize it from the current terminal via `ghostty_key_encoder_set_from_terminal`. The current encoder header says this imports cursor-key application, keypad mode, backarrow mode, ignore-keypad-with-numlock, Alt escape prefix, modifyOtherKeys and Kitty keyboard protocol flags from terminal state. See [key encoder header](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h).

That API eliminates a large category of bugs: the app does not maintain a shadow copy of terminal input modes.

### Mouse encoder

Use `GhosttyMouseEncoder` only once mouse reporting enters scope. The Ghostty example configures tracking mode, format, screen/cell geometry and event position before encoding. See [mouse encoder example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c). The application remains responsible for deciding whether a mouse gesture is local selection/scrollback or terminal-reported input.

### Paste

Use `ghostty_terminal_paste` for clipboard paste. Ghostty's paste example shows bracketed-paste handling, unsafe-paste rejection/confirmation, MIME selection and newer paste-event behavior. See [paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c). The MVP needs only text clipboard content, but using the terminal-aware API from the beginning avoids a later correctness rewrite.

## 3. What we explicitly do not use

Do not use full `ghostty_app_t` / `ghostty_surface_t` runtime as the main architecture. Wintty's minimal Win32 example shows what that model looks like: an `HWND` is handed to Ghostty, Ghostty creates a surface with DX12 rendering and ConPTY, and the host forwards Windows messages. See [Wintty Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c). That is appropriate when Ghostty owns rendering; it conflicts with our goal that GPUI/Silvestris own the one UI scene.

Do not copy Ghostty's Metal/OpenGL/DirectX renderer. GPUI already shapes text and submits its own platform scene. A second terminal renderer creates composition/synchronization ownership that is larger than the entire MVP.

Do not import Ghostty's app configuration system, shell integration, quick terminal, tabs/splits, notification system, shader config or platform app runtimes.

Do not depend on internal Zig structs under `src/terminal` from Rust. Use the public C API. Internal source is valid implementation research, but not a stable integration boundary.

## 4. FFI boundary

The Rust project should have one unsafe boundary. A safe wrapper might expose:

```rust
pub struct Terminal { raw: NonNull<GhosttyTerminalImpl> }
pub struct RenderState { raw: NonNull<GhosttyRenderStateImpl> }
pub struct KeyEncoder { raw: NonNull<GhosttyKeyEncoderImpl> }
```

The precise raw types depend on bindings. Each wrapper:

- is non-`Copy`;
- frees exactly once;
- converts `GhosttyResult` into a Rust error;
- validates nonzero dimensions and buffer lifetimes;
- does not expose borrowed C pointers outside their documented lifetime;
- only implements `Send`/`Sync` when the C API and session serialization make that sound.

Prefer methods such as:

```rust
terminal.write_vt(&[u8])?;
terminal.resize(GridSize, CellPixels)?;
render.begin_update(&terminal)?;
render.end_update()?;
```

instead of allowing the rest of the app to call raw functions.

The wrapper must not hide terminal serialization. Either methods require `&mut self` behind one mutex or the higher `TerminalSession` owns the lock. Do not scatter independent mutexes around terminal, encoder and render state in a way that creates lock ordering complexity.

## 5. Terminal callbacks/effects

A real terminal needs effects because terminal programs can ask the emulator to perform or report external state. Configure only the effects the MVP needs.

### `WRITE_PTY`

This is mandatory for protocol responses. Enqueue bytes into the ordered PTY writer queue. The callback is invoked in terminal processing context; do not block, do not call GPUI, and do not recursively write VT data into the same terminal.

### Size callback

Report accurate rows/columns and cell dimensions from the latest terminal geometry when applications query terminal size. Keep the geometry in session state accessible without grabbing GPUI objects from the reader thread.

### Title changed

Store/copy the new title into a thread-safe pending value and wake GPUI. The foreground view updates `WindowFrame`/window title. Never invoke GPUI window methods from the PTY reader thread.

### Bell

MVP may no-op or issue a minimal platform beep. Do not add notification banners just for BEL.

### Render hold

Honor synchronized-output holds by suppressing intermediate paints while continuing to parse bytes. Ghostty's terminal documentation explains render holds around mode 2026 and notes resize ends a hold. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

### Clipboard effects

Program-initiated clipboard read/write has security implications and is not required for the first terminal. Leave it unconfigured/denied until there is an explicit policy. User-initiated copy/paste is separate.

## 6. Rendering API details

The render state is not merely a “copy of the screen”; it is an incremental bridge. Relevant current data include:

- columns/rows;
- global dirty state;
- row iterator;
- colors/palette;
- cursor bundle;
- overscan;
- row dirty state;
- row cells;
- row-local selection;
- viewport row;
- row identity;
- per-cell style;
- per-cell grapheme data;
- resolved foreground/background colors.

See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Use `ghostty_render_state_row_iterator_next_dirty`. If global state is partial, it skips clean rows; if full, it yields all effective rows. The renderer should not independently diff terminal text strings.

Use row-local selection once per row. The header explicitly notes this avoids one C API call per cell. If FFI overhead is measurable, use `*_get_multi` helpers where they reduce crossings, especially for style/grapheme/color values. Do not prematurely build a custom shared-memory API.

## 7. Two-phase update and locking

Production path:

```rust
let generation = session.generation();

{
    let terminal = session.terminal.lock();
    render_state.begin_update(&terminal)?;
} // unlock

render_state.end_update()?;
```

Then the UI can inspect render state without holding the terminal lock. Ghostty documents that `end_update` exclusively reads/writes memory owned by render state and is safe while terminal mutation resumes. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

This is a major reason to prefer the VT render-state API over taking full screen snapshots. The UI should never hold the terminal lock through GPUI `shape_line` or `paint`.

## 8. Dirty-state lifecycle

A correct sequence is:

1. PTY writes mutate terminal and its dirty tracking.
2. UI begin/end update consumes terminal/screen dirty state into render state.
3. UI consumes every dirty row needed into its row cache.
4. UI marks render state clean.
5. Later terminal mutations create new terminal dirty state and generation.

If step 3 fails, do not clean. If the entire renderer cache is invalidated for an external reason such as font/DPI/theme changes, force/rebuild visible rows independently of Ghostty dirty state; Ghostty only knows terminal content changed, not that the UI font changed.

## 9. Resize

Call:

```text
ghostty_terminal_resize(
    cols,
    rows,
    cell_width_px,
    cell_height_px
)
```

only when terminal geometry changes. Current Ghostty documentation says resize updates pixel dimensions used for image protocols/size reports, disables synchronized output to show resize immediately, and sends in-band size reports when enabled. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Perform platform PTY resize in the same logical operation, with a documented ordering. Suggested order:

1. update session geometry;
2. resize Ghostty terminal;
3. resize OS PTY;
4. request render refresh.

If experimentation finds OS PTY first avoids a real issue, record why. The critical rule is that the two representations converge atomically from the app's perspective and stale resizes are coalesced.

## 10. Scrollback policy

Current Ghostty exposes both maximum bytes and maximum physical lines. The byte cap is approximate because history is stored/pruned in page-sized units; the header notes current page granularity is large enough that tiny byte settings round up substantially. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Therefore:

- choose an explicit line cap for user expectations;
- choose a byte cap for worst-case styled/grapheme-heavy output;
- expose both in metrics;
- do not attempt to enforce scrollback by deleting rows outside Ghostty.

If memory remains a problem in long sessions, measure Ghostty's memory-usage data and evaluate incremental compression. Compression is serialized terminal work and should run only when idle.

## 11. Terminal identity

The fact that Ghostty parses a protocol does not mean this app implements every external effect/render extension Ghostty's own GUI supports. Do not identify as full Ghostty or advertise its complete terminfo initially.

MVP recommendation:

```text
TERM=xterm-256color
COLORTERM=truecolor
```

and preserve the user's locale/environment. Revisit `TERM` only after a capability matrix proves enough compatibility and required terminfo is packaged. This avoids TUIs sending image/extension sequences the GPUI renderer intentionally ignores.

## 12. Graphics protocols

Kitty graphics is implemented end to end; Sixel is not.

The render state's image-placement iterator is the renderer's **sole** source of
placement geometry. Ghostty already resolves fixed placements, Unicode
placeholders, relative-placement chains, scrolling, resize, clipping and source
rectangles; Pain does not reimplement any of that.

The pipeline is:

- a process-global, bounded PNG decoder installed before the first terminal;
- `GHOSTTY_RENDER_STATE_DATA_IMAGE_PLACEMENTS_GENERATION` as the placement-cache
  key (never the storage generation, because scrolling moves placements without
  changing storage);
- one reusable `GhosttyRenderStateImageIterator`, rebuilt only when that
  generation changes and sorted by `(layer, z)`;
- an owned `ImagePlacement` list in `FrameMeta`, never attached to the row
  cache;
- a batched `TerminalSession::read_kitty_images()` that copies raw pixels under
  one terminal lock and skips generation races;
- a GPUI texture cache keyed by `(protocol, image_id, generation)` in
  `src/terminal/images.rs`, with explicit `Window::drop_image()` eviction;
- one demand-driven `tick_kitty_animation()` clock, scheduled by `TerminalView`
  with epochs rather than a frame loop.

Paint order is fixed:

`Kitty BELOW_BACKGROUND -> terminal/cell backgrounds -> Kitty BELOW_TEXT ->
shaped text -> Kitty ABOVE_TEXT -> cursor -> IME preedit -> scrollbar`.

Source rectangles are projected, not cropped: the full image is placed behind
the destination and `Window::paint_image` derives the atlas subregion from the
difference between `image_bounds` and the clipped `bounds`. Terminal/device
pixels are divided by the window scale; image pixels never are.

Transport policy is explicit and finite: direct inline transfer is the portable
default, the arbitrary-file medium is disabled by default, the temp-file medium
requires an allowed directory, and shared memory is only enabled where the host
namespace is meaningful. On Windows, a bundled, pinned OpenConsole
`conpty.dll` is preferred because the in-box host strips unknown APCs; the
`kernel32` fallback is logged as Kitty-graphics-degraded.

## 13. Unknown/custom sequences

Ghostty can expose unknown sequences/effects. The MVP should log unknown/unsupported behavior at debug level with rate limiting when useful for testing, not flood logs during normal operation. Do not implement custom control sequences until a concrete terminal program requires them.

## 14. Build/package approach

Ghostty's examples are standalone projects that build with Zig even when using the C API. See [Ghostty examples README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/README.md). The application has two reasonable packaging choices:

**Static library built in build tooling** — invoke a pinned Ghostty/Zig build step and link the resulting VT library. This produces simple runtime deployment but makes Rust builds depend on Zig.

**Prebuilt/pinned native artifact per target** — package a known VT library with the app and link/load it. This simplifies end-user build but adds artifact provenance/release work.

For the engineering demo, prefer reproducibility over cleverness: keep a script/build step that fetches/builds the pinned VT tag and records the exact revision. Do not use a system-installed arbitrary `libghostty-vt` whose ABI may differ.

## 15. Failure handling

Every C result must be checked. Treat OOM/invalid/unsupported differently where useful. An unsupported optional data key should disable only that optional feature; failure to create terminal/render state is fatal to session creation.

Never continue after FFI wrapper invariants fail. A partially constructed terminal session should unwind and close PTY/process handles cleanly.

## Deliverables

- Narrow, safe Rust wrapper over selected `libghostty-vt` release.
- Exact header/library version pin.
- Terminal/render/key/paste APIs integrated.
- Required terminal effects bridged without UI-thread or lock reentrancy.
- Explicit scrollback policy.
- Clear unsupported graphics/clipboard effects.
- Headless tests against the exact linked library.

## Avoidables

- Full Ghostty runtime/surface.
- Ghostty GPU renderer.
- Direct access to Zig internals.
- Tracking `main`.
- Reimplementing modes, parser or scrollback.
- Per-frame FFI object construction.
- Per-cell selection queries.
- Terminal callback directly touching GPUI.
- Assuming all Ghostty-supported protocols are rendered by this app.

## Regression traps

- Header/library ABI drift.
- Accidental `Send`/`Sync` on raw handles.
- Render-state clean before cache capture.
- Encoder not synchronized from terminal state.
- Recursive terminal access from effect callback.
- Scrollback compression colliding with reader/render access.
- Cell pixel dimensions omitted on resize.
- New Ghostty release changing enum/struct size assumptions.
- Enabling graphics protocol storage without a finite cap or without the generation-based renderer cache.
- `TERM` identity drifting ahead of capability.

## Source anchors

- Ghostty: [README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)
