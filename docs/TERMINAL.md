# Terminal Architecture

This document defines the application runtime: what a terminal session owns, how bytes move between the child process and Ghostty, how render state crosses into GPUI, how input travels back to the PTY, and where platform-specific code stops. The design is intentionally for **one session in one window**. The abstractions should be correct enough to permit more sessions later, but no code should be generalized merely because tabs or splits might exist someday.

The core boundary follows Ghostty's public VT API. A `GhosttyTerminal` owns terminal-emulator state—screen, scrollback, cursor, styles and modes—but Ghostty explicitly leaves drawing and PTY transport to the embedder. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Silvestris says application/domain views own their models and app-specific input policy while generic chrome/focus remain toolkit concerns, so `TerminalSession` belongs to the application rather than Silvestris. See [Silvestris consumer contract](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md). GPUI's custom `Element` lifecycle gives the renderer a low-level, single-element paint surface. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

## 1. Responsibilities

### `TerminalSession`

`TerminalSession` is the source of truth for one running terminal session. It should contain or coordinate:

```rust
struct TerminalSession {
    terminal: Mutex<GhosttyTerminal>,
    render: GhosttyRenderState,
    key_encoder: GhosttyKeyEncoder,
    mouse_encoder: GhosttyMouseEncoder,
    pty: PlatformPty,
    generation: AtomicU64,
    wake_pending: AtomicBool,
    lifecycle: SessionLifecycle,
    geometry: TerminalGeometry,
    metrics: TerminalMetrics,
}
```

This is illustrative, not a required public API. In particular, a safe Ghostty wrapper may need to separate `Send`/`Sync` ownership from the raw C handle. The important invariant is that **all mutation/inspection of one `GhosttyTerminal` is serialized**, while `GhosttyRenderState` becomes independently readable after `ghostty_render_state_begin_update`/`end_update`.

The session owns terminal semantics and transport state. It does not own window chrome, GPUI scene data, theme globals, tabs, profiles or preferences.

### `TerminalView`

`TerminalView` is the GPUI entity/view that:

- owns a `FocusHandle`;
- holds a reference/entity to `TerminalSession`;
- translates GPUI mouse/key/text/scroll events into terminal actions;
- requests repaint when the session generation changes;
- builds one `TerminalElement`;
- owns only UI-local state such as hover/selection gesture phase and cursor blink phase.

It should not parse VT, know ConPTY/FD details, or contain a second copy of terminal cells.

### `TerminalElement`

`TerminalElement` is the custom low-level GPUI element. It:

- requests layout;
- computes actual content bounds and terminal grid geometry;
- obtains an updated Ghostty render snapshot;
- refreshes only dirty row caches;
- registers hitboxes/input handler in prepaint;
- paints backgrounds, text, selection and cursor.

It should not spawn or block on I/O. It should not hold the terminal lock while shaping or painting text.

### `PlatformPty`

The common platform interface should remain small:

```rust
trait PlatformPty {
    fn write(&self, bytes: Arc<[u8]>) -> Result<()>;
    fn resize(&self, cells: CellSize) -> Result<()>;
    fn request_close(&self);
    fn process_id(&self) -> Option<u32>;
}
```

The actual implementation can expose channel senders rather than synchronous methods. The key point is that UI/common code asks for **bytes, resize, close**, not Win32/POSIX operations. Windows handles and Unix file descriptors never escape the platform module.

## 2. Byte flow from child to screen

The read path is:

```text
child stdout/stderr through PTY
        │
        ▼
blocking platform reader thread
        │ read 16–64 KiB chunk
        ▼
lock GhosttyTerminal
        │
        ├─ ghostty_terminal_vt_write(chunk)
        │    ├─ updates grid/modes/cursor/scrollback
        │    └─ may invoke effects such as WRITE_PTY/title/bell
        │
        └─ unlock
        ▼
generation += 1
        │
        └─ schedule one GPUI wake if none already pending
```

The exact read buffer size is a tuning value, not a protocol contract. Do not emit one terminal write per byte. `ghostty_terminal_vt_write` is built to consume byte streams; Ghostty's `c-vt-stream` example feeds byte ranges directly. See [Ghostty stream example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-stream/src/main.c).

A terminal effect may need to send a response back to the child while parsing, for example device/status queries. `GHOSTTY_TERMINAL_OPT_WRITE_PTY` exists for this. The callback executes during terminal processing and must preserve ordering relative to other terminal responses. It must **enqueue** output to the PTY writer; it must not call a potentially blocking OS write while holding the terminal serialization lock. See [Ghostty terminal effects/options](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 3. Writer path

Use one writer path per session:

```text
key / paste / Ghostty terminal effect
        │
        ▼
ordered writer queue
        │
        ▼
blocking writer thread
        │
        ▼
PTY input
        │
        ▼
child stdin
```

The queue must preserve ordering. A response generated during parsing must not race ahead of or behind user input in a way that changes the terminal protocol. The queue should be bounded by policy or at least expose queue-depth instrumentation. For one terminal, `std::sync::mpsc` or another already-present small channel is sufficient if it does not force an async runtime.

Large pastes should be handled without allocating a pathological number of tiny queue entries. The Ghostty paste API supports streamed clipboard reads; the application may collect moderate text into one buffer for MVP, but it should not create one message per character.

## 4. GPUI wakeup/coalescing

PTY output can arrive much faster than a display can render. The reader must never translate “one read completed” into “one independently queued GPUI callback.” Maintain:

- `generation: AtomicU64` — increment whenever terminal state may have changed;
- `wake_pending: AtomicBool` — indicates a foreground wake has already been scheduled.

Producer pseudocode:

```text
write terminal state
generation.fetch_add(1)

if wake_pending.compare_exchange(false, true) succeeds:
    schedule foreground refresh
```

Foreground consumer:

```text
observe current generation
mark view/entity dirty
clear wake_pending
re-check generation/pending race
schedule again only if output arrived after the consumed generation
```

The exact race-free sequence should have a unit test. An acceptable alternative is a channel with capacity one where send failure means “a wake is already queued.” What is not acceptable is an unbounded queue of UI wakeups.

Zed's terminal at the same GPUI revision uses backend `Wakeup` events and deliberately batches events after the first low-latency event, providing useful precedent that terminal output should be coalesced before entity updates. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

## 5. Render-state handoff

Ghostty's render API is designed for exactly the synchronization problem this application has. `ghostty_render_state_begin_update` reads terminal state while the caller holds its synchronization primitive. After that call returns, release the terminal lock. `ghostty_render_state_end_update` only touches render-state-owned memory and can run while the terminal is being modified. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Recommended frame flow:

```text
TerminalElement::prepaint
    │
    ├─ compare session generation with last captured generation
    │
    ├─ if changed:
    │    lock terminal
    │    ghostty_render_state_begin_update(...)
    │    unlock terminal
    │    ghostty_render_state_end_update(...)
    │
    ├─ inspect global dirty status
    │
    ├─ iterate dirty rows only
    │
    ├─ rebuild row caches
    │
    └─ record frame data

TerminalElement::paint
    │
    ├─ paint cached quads
    ├─ paint cached/shaped row text
    ├─ paint cursor
    └─ after complete consumption:
         ghostty_render_state_clean(...)
```

If the implementation finds it cleaner to call `clean` after cache construction rather than after GPU submission, that can be valid because the cache has become the complete frame source. The invariant is that dirty state is not cleared until every changed row needed for the frame has been safely captured. Document the chosen point.

Ghostty exposes `DIRTY_FALSE`, `DIRTY_PARTIAL`, and `DIRTY_FULL`. `next_dirty` skips clean rows for partial updates and returns all effective rows for full updates. See [Ghostty render API](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h) and [render example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-render/src/main.c).

## 6. Row cache

A row cache should hold render-ready information rather than raw Ghostty handles:

```rust
struct CachedRow {
    row_id: u64,                    // if useful from Ghostty row identity
    backgrounds: SmallVec<[BgRun; 4]>,
    text_runs: Vec<PreparedTextRun>,
    selection: Option<RangeInclusive<u16>>,
    content_hash_or_generation: ...,
}
```

Do not assume this exact representation. The important properties are:

- unchanged rows are retained;
- row rebuild is linear in cells of that row;
- adjacent cells with compatible style are merged;
- no raw FFI pointer survives beyond documented lifetime;
- the cache invalidates on font/scale/geometry changes even if Ghostty reports content clean.

Ghostty provides UTF-8 grapheme data per cell and row-level selection ranges. Prefer the row-level selection query over asking every cell for `SELECTED`; the header itself documents that policy as avoiding one C call per cell. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

## 7. Text batching

A terminal row is not one style, but it also should not be one paint call per cell. Merge adjacent cells when all properties relevant to GPUI shaping/painting match:

- font family/face selection;
- bold/italic mapping;
- foreground color;
- underline/strikethrough treatment when GPUI run representation requires it;
- any terminal decoration that cannot be painted separately;
- no discontinuity caused by wide-cell continuation rules.

Background color does not necessarily need to split a text run if backgrounds are painted independently. That choice should be benchmarked, but keeping background quads separate often permits larger text batches.

Zed's pinned terminal has an explicit `BatchedTextRun` “that combines multiple adjacent cells with the same style.” Treat this as validation of the batching strategy. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs). GPUI's `TextRun` and `shape_line` are the underlying shaping primitives. See [GPUI `text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

## 8. Unicode and cells

Never implement terminal layout by iterating Rust `char`s and incrementing `x` by one. Ghostty already tracks graphemes and terminal-cell occupancy. The renderer consumes the terminal's cell structure, not a plain string reconstruction.

Wide glyphs, combining marks, zero-width components, emoji and variation selectors are exactly why the terminal grid must stay authoritative. The GPUI shaped output must be positioned against terminal cell geometry even when the font's natural advance differs. If GPUI shaping produces an advance wider than expected, do not silently let later terminal cells drift; terminal columns remain fixed. Any stretch/clipping/fallback policy should be localized to the text painter and tested with fixtures.

## 9. Geometry and resize

`TerminalGeometry` should separate logical GPUI dimensions from terminal cell dimensions:

```text
content_bounds logical px
device scale
font/cell width logical px
line height logical px
cols = floor(content_width / cell_width)
rows = floor(content_height / line_height)
cell_width_px = round(cell_width * scale)
cell_height_px = round(line_height * scale)
```

Minimum rows/columns should never become zero when a window is temporarily very small; clamp to at least `1×1` for terminal APIs and suppress painting outside actual bounds.

On geometry change:

1. recompute rows/columns and cell pixel dimensions;
2. if nothing terminal-relevant changed, do not resize;
3. call `ghostty_terminal_resize`;
4. request platform PTY resize with rows/columns;
5. invalidate row caches because reflow/line mapping may change.

Ghostty's resize API takes rows/columns and cell pixel dimensions and updates size-report/image protocol state. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Zed's terminal explicitly avoids PTY resize spam for pixel-level changes that do not alter grid/cell geometry. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

## 10. Keyboard input

There are two input classes:

**Physical terminal keys**: arrows, function keys, Home/End, Ctrl combinations, keypad, Escape, Backspace and similar. Translate GPUI key data into `GhosttyKeyEvent`, synchronize `GhosttyKeyEncoder` from the current terminal with `ghostty_key_encoder_set_from_terminal`, encode, and enqueue bytes. Ghostty tracks cursor-key application, keypad application, Alt prefix, modifyOtherKeys, Kitty keyboard flags and backarrow mode. See [Ghostty key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h).

**Committed text**: Unicode text generated by normal typing, IME/dead-key composition or text services. Receive it through GPUI's native input-handler mechanism, then send it as text input rather than trying to reverse-engineer it from physical keycodes. GPUI documents `InputHandler` as the bridge from platform-native text protocols and uses UTF-16 ranges unless stated otherwise. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

Avoid double emission. A printable key may generate both a key event and committed text depending on GPUI/platform behavior. Define which path owns ordinary printable characters and test Windows/Linux separately.

## 11. Focus and command routing

The terminal must own focus whenever the user clicks its canvas or the window opens with terminal focus. `TerminalView` should implement/use GPUI `FocusHandle`; GPUI describes it as the mechanism for tracking/manipulating keyboard focus. See [GPUI `app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs).

Silvestris's responder chain should treat the terminal as a domain canvas. Its `capture_keys` helper is explicitly intended for a focused canvas that consumes almost all commands while allowing selected shell shortcuts outward. See [Silvestris `responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs). Define an allowlist rather than a denylist. Candidate app-level commands are close/quit and copy/paste only when terminal policy explicitly delegates them; do not let generic text-editing key bindings consume terminal Ctrl sequences.

## 12. Paste

Clipboard paste is a terminal protocol operation, not simply `pty.write(clipboard_bytes)`. Ghostty's terminal-aware paste API applies bracketed-paste mode and can detect unsafe control/newline content. Its C example demonstrates:

- normal paste;
- refusing unsafe unbracketed newline input;
- retry after confirmation;
- bracketed paste mode 2004;
- newer paste event behavior.

See [Ghostty paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c).

For the minimal app, support normal text clipboard content. If Ghostty returns “rejected/unsafe,” show a minimal confirmation surface using Silvestris rather than silently stripping or sending. Do not implement image clipboard paste.

## 13. Copy and selection

Ghostty owns selection positions because selection must survive scrollback/grid semantics. Mouse hit testing converts GPUI coordinates to cell coordinates, then uses Ghostty grid references/selection APIs. The renderer queries row-local selection ranges from render state to draw selection.

Copy requests terminal selection text through Ghostty's formatter/selection APIs rather than concatenating cached visual runs. Cached runs are rendering artifacts and may not preserve newline/wrap semantics correctly.

Selection must continue to work after scrollback movement and resize. Avoid storing selection only in pixel coordinates.

## 14. Scrollback

Scrollback belongs entirely to `GhosttyTerminal`. The terminal API exposes viewport movement and scrollbar state. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Wheel behavior:

- ordinary shell output: wheel moves Ghostty viewport;
- alternate-screen app with terminal mouse/alternate-scroll semantics: use the active terminal modes to decide whether wheel is reported to the application;
- explicit local-selection override: moves/local selects without reporting as defined by app policy.

If an Aqua scrollbar is displayed, it reflects Ghostty `total/offset/len` and calls Ghostty viewport APIs. Silvestris's existing `Scrollbar` is designed around a GPUI `ScrollHandle` and therefore cannot simply be attached to terminal history as-is. See [Silvestris `controls/scrollbar.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/controls/scrollbar.rs). Either create a thin terminal-specific adapter in the application or initially omit a visible scrollbar; do not duplicate the history model.

## 15. Cursor and blink

Cursor shape/visibility comes from Ghostty render state. Blink timing is application/UI state. Silvestris exposes the system caret blink interval on Windows and a designed fallback elsewhere through `native::caret_blink`; using it is reasonable for Aqua feel. See [Silvestris `native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs).

Cursor blinking should not cause a full terminal render-state update or row reshape if content did not change. Cache cursor position/style from the latest render snapshot and repaint only the cursor layer/frame.

## 16. Terminal effects

Implement only effects required for correctness or minimal polish:

**Required/early**
- `WRITE_PTY`: protocol replies;
- title changed: update app/window title;
- size report: return accurate rows/cols/cell pixels;
- bell: minimal/no-op or system bell policy;
- render hold/synchronized output if exposed by the selected VT version.

**Later/explicit**
- working-directory metadata;
- progress/program status;
- notifications;
- clipboard read/write initiated by the child;
- window resize requests from the child;
- semantic prompts;
- image protocols.

Ghostty's effect documentation warns that programs can ask questions that need replies. Unknown/unsupported effects should be deliberately ignored or denied, not accidentally half-implemented. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 17. Synchronized output

Ghostty exposes render-hold behavior for synchronized output mode (2026). The application should avoid painting intermediate terminal states during an active hold, but it must still process PTY bytes. Resize ends a hold per Ghostty's documented behavior. Keep this as a small boolean in session/render scheduling rather than building a separate buffering system; Ghostty already owns terminal mutation.

## 18. Scrollback limits and memory

Do not rely blindly on Ghostty's defaults. The current terminal header exposes byte and line scrollback caps. It notes byte pruning happens at internal page granularity and a page is roughly hundreds of KiB at the time of those docs. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Set a modest explicit MVP policy after measuring normal use—for example a line cap suitable for a small terminal plus a byte cap. The exact numbers should be configuration constants, not hard-coded throughout the renderer. Record memory-usage counters during long sessions. Ghostty also exposes optional compression, but compression is a later optimization and is not thread-safe with other terminal operations; schedule only after measurement.

## 19. Lifecycle

Suggested states:

```text
Starting
  -> Running
  -> ChildExited(status)
  -> Closing
  -> Closed
```

Initialization failures should become a visible minimal error in the same window or terminate with a clear error; do not leave a half-created session.

Closing order must be platform-specific but conceptually:

1. stop accepting new input;
2. signal/close child PTY according to platform policy;
3. wake/join writer;
4. unblock/join reader;
5. reap child/process;
6. release platform PTY handles;
7. release Ghostty encoders/render state/terminal;
8. let GPUI entity/window drop.

Do not block the GPUI foreground thread while joining potentially stuck I/O threads. Use a short asynchronous/background cleanup path and ensure OS handle closure unblocks reads.

## 20. Test matrix

### Headless terminal tests

- ANSI style and reset.
- 16/256/true-color cells.
- alternate screen enter/exit.
- cursor move/style/visibility.
- scrollback and viewport movement.
- bracketed paste mode.
- key encoder in normal/application cursor modes.
- wide/combining text.
- resize/reflow.
- synchronized output hold.

### Renderer tests

- row run batching.
- clean frame does no row rebuild.
- partial dirty frame rebuilds only changed rows.
- full dirty frame rebuilds all visible rows.
- cursor blink does not rebuild rows.
- font/DPI change invalidates caches.
- selection row ranges map to exact quads.
- wide-cell continuation does not produce duplicate text.

### Concurrency tests

- wakeup coalescing race.
- output arrives during render-state end/update.
- title/write-PTY effects preserve ordering.
- writer queue closes cleanly on child exit.
- no deadlock when close races with output.

### Interactive tests

- Windows: cmd, PowerShell, vim, TUI mouse app, large output.
- Linux: user shell, vim, htop/btop, large output.
- Both: resize while output streams; copy/paste after long scrollback; terminal app enters/exits alternate screen repeatedly.

## Deliverables

- One shared `TerminalSession` architecture on Windows/Linux.
- One shared custom GPUI renderer.
- Strict terminal serialization and short render lock handoff.
- Ordered nonblocking PTY write path.
- Coalesced GPUI wakeups.
- Mode-aware keyboard/mouse/paste.
- Ghostty-owned scrollback/selection.
- Deterministic shutdown.

## Avoidables

- Duplicate terminal models.
- Full-screen snapshots copied for every frame.
- OS I/O on foreground thread.
- Per-cell UI nodes.
- Per-cell selection FFI query.
- Renderer parsing control sequences.
- PTY backend interpreting terminal modes.
- UI cache becoming source of truth.
- Generic async runtime before need.
- Multiple sessions before one session is robust.

## Regression traps

- A “clean” Ghostty render state but stale row cache after font/DPI change.
- Lost UI wakeup between `wake_pending` clear and generation recheck.
- Deadlock through synchronous `WRITE_PTY`.
- Key encoder not resynchronized after terminal mode changes.
- Paste bypassing bracketed mode.
- Scrollback view and selection disagreeing after resize.
- Cursor-only animation forcing full shape/layout.
- Process exit not unblocking PTY reader.
- Resize ordering mismatch between Ghostty and OS PTY.
- Title/effect callbacks trying to mutate GPUI from reader thread directly.

## Source anchors

- Ghostty: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h), [paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs), [`native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)
