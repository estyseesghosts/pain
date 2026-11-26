# Performance Plan

The goal is not to beat Ghostty's standalone renderer in synthetic benchmarks. The goal is to make a one-window Silvestris terminal feel immediate and remain stable during large output, live resize and long sessions without importing the overhead that frustrates the current Wintty/WinUI experience. Performance work should focus on the layers this project adds: FFI crossings, terminal serialization, wakeups, row-cache construction, GPUI text shaping/painting, PTY I/O and resize behavior.

Ghostty already invests heavily in parser/terminal performance; its README describes a dedicated read/write/render architecture and SIMD terminal parser in the full application. See [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md). We are deliberately reusing that terminal engine and should not optimize inside it before measuring our own boundary. Silvestris is a GPUI consumer toolkit and already pins release settings such as `opt-level=3`, thin LTO and one codegen unit. See [Silvestris `Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml). GPUI exposes custom elements/text shaping so the terminal can avoid a huge child-element tree. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs) and [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

## 1. Performance model

The critical output path:

```text
OS PTY read
 -> Ghostty parse/mutate
 -> coalesced wake
 -> Ghostty render-state begin/end
 -> dirty rows
 -> row cache
 -> GPUI shape dirty runs
 -> GPUI paint quads/text
 -> present
```

The critical input path:

```text
GPUI key/text
 -> Ghostty mode-aware encode/paste
 -> writer queue
 -> OS PTY write
```

The terminal should be idle when nothing changes except a low-frequency cursor blink. No polling loop should continuously parse/render.

## 2. Project targets

These are **engineering targets**, not upstream benchmark claims.

- Ordinary keystroke-to-visible-echo should normally fit within one display frame plus child-shell processing.
- Terminal-specific foreground work should avoid individual blocking stalls above roughly 2 ms under ordinary interactive load; investigate any repeated longer stall.
- Sustained output should not create an ever-growing foreground callback queue.
- Cursor blink should not reshape terminal rows.
- A one-pixel window drag that does not change rows/columns should not resize the PTY.
- Clean frames should produce zero row rebuilds.
- Partial dirty frames should rebuild approximately the number of dirty rows, not all visible rows.
- Long-session memory should plateau according to configured Ghostty scrollback limits rather than grow with every line forever.
- Closing should not wait indefinitely on reader/writer threads.

Do not turn these into hard sleeps/frame caps. They are profiling signals.

## 3. Required counters

M2 implements these as cheap `AtomicU64` counters in
`src/terminal/session.rs`; `TerminalSession::snapshot_counters()` returns a
point-in-time `CountersSnapshot`, and a `CountersHandle` lets the GPUI element
record rows rebuilt/cache hits, text batches, `shape_line` calls, background
quads and prepaint/paint time without borrowing the session. The headless
stress tests in the same module assert wakeup coalescing and bounded lock hold
time.



Keep counters behind a debug/perf feature or cheap atomics so they can be enabled during development:

### PTY

- bytes read;
- read calls;
- bytes written;
- write calls;
- writer queue messages;
- maximum writer queue depth;
- child exits;
- read/write errors.

### Terminal

- `ghostty_terminal_vt_write` calls;
- bytes per VT write;
- total time in VT write;
- maximum VT write duration;
- terminal lock acquisitions;
- cumulative/max lock hold duration;
- resize calls;
- scrollback memory stats if available.

### Wakeups

- output generations;
- wakeup attempts;
- wakeups actually scheduled;
- wakeups consumed;
- coalesced/dropped-as-already-pending wakeups.

A healthy output burst should show many PTY reads but far fewer GUI wakeups.

### Render

- render-state begin/end calls;
- dirty state full/partial/clean counts;
- dirty rows visited;
- rows rebuilt;
- row cache hits;
- text batches generated;
- `shape_line` calls;
- total shaped UTF-8 bytes;
- background quads;
- cursor-only frames;
- terminal prepaint time;
- terminal paint time.

### Kitty graphics

Kitty work has its own generation-based model; it is never attached to the row
dirty mask. `CountersSnapshot` reports:

- `image_placement_list_rebuilds` / `image_placement_cache_hits` — placement
  iterator calls only when `IMAGE_PLACEMENTS_GENERATION` changes;
- `image_pixel_reads` / `image_pixel_bytes_copied` — raw copies under one
  terminal lock, deduplicated by `(image_id, generation)`;
- `image_cache_misses` / `image_generation_races` — missing textures and
  generations superseded between the render-state snapshot and the pixel read;
- `image_texture_uploads` / `image_texture_evictions` — GPUI atlas work;
- `image_copy_lock_max_micros` — worst terminal-lock hold during a pixel copy;
- `kitty_animation_ticks` / `kitty_animation_frames` — demand-driven animation
  clock, not a frame loop.

`tests/m6.rs` asserts these invariants:

- static image + cursor blink: zero placement rebuilds, zero pixel reads,
  zero texture uploads, zero row reshaping;
- static image + heavy text flood: no texture re-upload unless the image
  generation changes;
- image-only animation frame: zero row materialization and zero `shape_line`
  calls;
- placement scroll: placement-list rebuild allowed, no extra pixel upload when
  the image generation is unchanged;
- font/DPI resize: Ghostty geometry changes, unchanged grid does not resize the
  OS PTY;
- image deletion/generation change: the old GPUI texture is evicted;
- a long image session stays bounded by the configured storage cap.

The `m6` tests print one `M6 <case>:` line per scenario; capture those lines
from a release build with:

```sh
cargo test --release --test m6 -- --nocapture
```

Representative Windows GNU release results (one run; timings are wall-clock
microseconds and vary with machine load):

| Case | Key evidence |
|---|---|
| output flood | 8000 writes / 216000 bytes -> 1 scheduled wakeup, 55 render updates, max terminal lock 693 us |
| static image + cursor | 1 placement rebuild, 1 cache hit, 0 pixel reads, 0 uploads, 0 rebuilt rows, 0 `shape_line` calls |
| animation frame | 2 placement rebuilds, 1 tick / 1 frame, 0 rebuilt rows, 0 `shape_line` calls |
| placement scroll | 1 rebuild + 1 hit, 2 pixel reads (8 bytes), 0 uploads, copy lock max 88 us |
| DPI-only resize | 2 Ghostty geometry updates, 0 OS PTY grid resizes |
| resize storm | 10 Ghostty geometry updates, 9 coalesced OS PTY resizes |
| bounded images | 100 transmitted images under a 64-byte storage cap stay bounded; max terminal lock 726 us |

WSL/Linux release results for the same matrix (same machine family, different
scheduler and filesystem):

| Case | Key evidence |
|---|---|
| output flood | 8000 writes / 216000 bytes -> 1 scheduled wakeup, 126 render updates, max terminal lock 278 us |
| static image + cursor | 1 placement rebuild, 1 cache hit, 0 pixel reads, 0 uploads, 0 rebuilt rows |
| animation frame | 2 placement rebuilds, 1 tick / 1 frame, 0 rebuilt rows, 0 `shape_line` calls |
| placement scroll | 1 rebuild + 1 hit, 2 pixel reads (8 bytes), 0 uploads, copy lock max 1 us |
| DPI-only resize | 2 Ghostty geometry updates, 0 OS PTY grid resizes |
| resize storm | 10 Ghostty geometry updates, 9 coalesced OS PTY resizes |
| bounded images | 100 transmitted images under a 64-byte storage cap stay bounded; max terminal lock 44 us |

The WSLg GUI gate additionally ran the real Kitty client `kitten icat`
(kitty 0.47.1) through the window: a fixed PNG on the Unicode-placeholder path
and a looping four-frame GIF. The captured window showed the checkerboard image
on its placeholder cells and successive animation frames in different colors,
exercising client -> WSL PTY -> Pain -> libghostty-vt render-state placements ->
GPUI texture -> paint. `kitten` reads pixel geometry from `TIOCGWINSZ`, so Pain
reports the measured device cell size through `ws_xpixel`/`ws_ypixel` when the
PTY is created and when the grid changes; a DPI-only change never reaches the
PTY. XTWINOPS `CSI 14/16/18/19 t` is answered separately by
`GHOSTTY_TERMINAL_OPT_SIZE`. Placeholder cells are shaped as blanks: the
virtual placement supplies their pixels, so the fallback font cannot draw tofu
over or beside the image. `tests/pty.rs` repeats the real
`kitten icat --unicode-placeholder` gate; `tests/kitty.rs`, `tests/m6.rs` and
`tests/gpui_render.rs` cover placeholders, animation and the render path
deterministically.


### Resize

- GPUI bounds changes observed;
- computed grid-size changes;
- Ghostty resize calls;
- OS PTY resize calls;
- duplicate resize requests suppressed.

## 4. Logging policy

Do not log per byte/cell/read in normal debug builds. Perf counters can be dumped:

- on demand via a hidden debug shortcut;
- on process exit in debug builds;
- after a benchmark fixture.

Rate-limit warnings such as repeated resize failures or unsupported terminal effects.

## 5. Ghostty render-state usage

Use the two-phase update. Ghostty explicitly documents it so callers can hold their terminal lock only during `begin_update`, then complete work after releasing the lock. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Use `next_dirty` rather than walking every row after every update. Use row-level selection data rather than per-cell selection calls. Use `*_get_multi` only if profiling says FFI call overhead is meaningful.

Call `render_state_clean` only after dirty rows are safely captured.

## 6. Row-cache strategy

Start simple:

- `Vec<CachedRow>` sized to visible rows;
- each row owns reusable `String`/Vec buffers where possible;
- rebuild only dirty rows;
- retain capacity between rebuilds;
- merge adjacent backgrounds/styles.

Avoid elaborate hash-based caching until normal dirty flags prove insufficient. Ghostty already tells us what changed.

When resize/reflow/full-dirty occurs, full visible rebuild is expected. Do not attempt clever reuse across a geometry-changing reflow until measured.

## 7. Text shaping

Text shaping is likely one of the largest costs this app owns. Zed's pinned terminal code and the pinned commit itself are evidence that avoiding unnecessary text-run cloning/shaping matters in a terminal renderer. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs).

Rules:

- shape batched runs, not cells;
- do not shape empty/background-only cells;
- do not shape clean rows;
- reuse strings/buffers where possible;
- avoid cloning style vectors before `shape_line`;
- profile fallback-heavy Unicode separately;
- cache shaped row/run output only if GPUI's shaped-line lifetime/API makes it safe at the pinned revision.

Never bypass GPUI shaping with a second font rasterizer in MVP.

## 8. Quad count

Coalesce solid runs:

- base background: one fill for whole content;
- non-default backgrounds: one quad per contiguous same-color run;
- selection: one quad per contiguous row range;
- cursor: one/few quads.

A normal prompt should not create one background quad per cell.

## 9. Wakeup coalescing

An output flood such as `yes` can produce thousands of PTY reads. The GUI only needs the most recent terminal state by the next frame, subject to low interactive latency.

Use one pending wake flag or capacity-one channel. Consider a very small batching window only if needed. Zed's terminal batches follow-on events after the first prompt event, which validates the general approach. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

Do not deliberately sleep the reader thread to batch output; parse promptly and coalesce the UI notification.

## 10. Reader chunk size

Use a reusable buffer large enough to amortize syscalls but not huge enough to increase latency/memory. Start around 16–64 KiB and benchmark. The exact size is lower priority than avoiding allocation per byte/read and keeping terminal lock time reasonable.

If one giant read causes a long `vt_write` lock hold, cap chunk processing or split extremely large reads before parse. Measure first.

## 11. Writer queue

User typing is tiny and latency-sensitive; paste/replies may be larger. Preserve order without starving small user writes behind pathological queued data if that actually occurs. The simplest ordered queue is correct for MVP.

Track max queue depth. If it grows during ordinary use, find the cause rather than adding complex prioritization.

## 12. Resize storms

GPUI/Silvestris may render many intermediate window sizes during drag. Terminal work should occur only when terminal geometry changes. Zed's pinned `set_size` explicitly suppresses PTY resize when pixel-level changes leave row/column/cell geometry unchanged and has a regression test for it. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

Backend also remembers last applied grid size.

During resize:

- do not debounce so heavily that the terminal visibly lags;
- do coalesce duplicate sizes;
- allow Ghostty's resize/reflow to be the expensive operation, not dozens of redundant calls.

## 13. Cursor blink

Cursor blink frame should:

- toggle cursor paint visibility;
- repaint the view;
- reuse cached row text/quads.

Counter invariant:

```text
cursor_only_frames > 0
rows_rebuilt during those frames == 0
shape_line calls during those frames == 0
```

If this fails, fix before adding more animations.

## 14. Scrollback

Set explicit caps. Measure:

- current Ghostty terminal memory;
- resident set over long output;
- row-cache memory;
- shaped-text cache memory if any.

Ghostty supports optional scrollback compression but documents serialization/thread-safety constraints. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Do not enable compression until a long-session profile shows it is worthwhile.

## 15. Stress workloads

Use deterministic commands where possible.

### Parser/output

Linux examples:

```sh
seq 1 200000
yes "0123456789abcdef" | head -n 200000
find /usr -maxdepth 5 -type f 2>/dev/null
```

Windows PowerShell equivalents:

```powershell
1..200000 | ForEach-Object { $_ }
1..100000 | ForEach-Object { "0123456789abcdef" }
Get-ChildItem C:\Windows -Recurse -ErrorAction SilentlyContinue
```

Do not treat maximum line-print benchmark as the only performance test; interactive latency during output matters more.

### ANSI/style churn

Create a generated fixture that cycles:

- foreground/background;
- bold/italic/underline;
- cursor movements;
- erase operations;
- alternate screen;
- wide Unicode;
- true color.

### Full-screen TUI

- `vim`;
- `htop`/`btop` on Linux;
- a Windows TUI available locally;
- `opencode`/another agent CLI for long transcript behavior.

### Resize

While streaming output:

- drag window continuously;
- maximize/restore;
- move between different DPI displays on Windows if available;
- repeatedly switch narrow/wide sizes.

### Long session

Keep a shell/agent session open with substantial scrollback. Measure RSS, render counters and input latency after the history has grown.

## 16. Benchmark recording

For each performance investigation record:

```text
commit:
platform:
CPU/GPU:
display refresh:
font/size:
terminal size:
scrollback policy:
workload:
duration:
bytes read:
wakeups scheduled:
render updates:
dirty rows:
shape_line calls:
max terminal lock:
p95 terminal prepaint:
RSS start/end:
observed issue:
```

Do not compare results across different workloads/hardware without noting it.

## 16a. M6 measured stress suite (2026-11-25)

`tests/m6.rs` is the repeatable headless stress suite. Run it with counters
printed on both platforms:

```sh
# Windows (PowerShell), from pain/
cargo test --release --test m6 --features shell -- --nocapture

# Linux (bash), from a WSL ext4 working copy
cargo test --release --test m6 --features shell -- --nocapture
```

Windows host, release profile, 80x24 session, synthetic fixture lines, four
producer threads. Counters are `TerminalSession::snapshot_counters()`:

| Platform | Workload | Writes | Bytes | Wakeups scheduled / coalesced | Render updates | Dirty full / partial / clean | Max lock hold |
|---|---|---:|---:|---:|---:|---:|---:|
| Windows | Output flood | 8,000 | 216,000 | 1 / 7,999 | 60 | 60 / 0 / 0 | 210 us |
| Windows | Cursor-only frame | 2 | 15 | 1 / 1 | 2 | 1 / 0 / 1 | 97 us |
| Windows | Resize storm | 0 | 0 | 1 / 8 | 0 | 0 / 0 / 0 | 0 us |
| Linux (WSL ext4) | Output flood | 8,000 | 216,000 | 1 / 7,999 | 152 | 152 / 0 / 0 | 577 us |
| Linux (WSL ext4) | Cursor-only frame | 2 | 15 | 1 / 1 | 2 | 1 / 0 / 1 | 186 us |
| Linux (WSL ext4) | Resize storm | 0 | 0 | 1 / 8 | 0 | 0 / 0 / 0 | 0 us |

Linux renders more frames during the same flood because its producer is slower
relative to the renderer; wakeup coalescing and lock bounds still hold.

What these numbers prove:

- A flood of 8,000 writes schedules **one** UI wakeup; the other 7,999 are
  coalesced. No unbounded foreground callback queue is created.
- The cursor-only frame is `DIRTY_CLEAN` with zero dirty rows. Feeding the same
  dirty state through `RowCache::rows_to_rebuild` returns an empty list, and the
  counters show `rows_rebuilt == 0` and `shape_line_calls == 0`. Cursor blink
  therefore reshapes nothing.
- 1,000 resize requests with only nine distinct grid sizes produce ten Ghostty
  geometry updates (one pixel-only plus nine grid changes), nine PTY resize
  calls and 990 suppressed duplicates; pixel-only motion never reaches the PTY.
- Terminal lock hold stays in the low hundreds of microseconds because it only
  covers `ghostty_terminal_vt_write` or render-state `begin_update`, never
  shaping or painting.

## 16b. Repair-pass counters (latest-wins geometry/input work)

The original `rows_rebuilt` number was too coarse: it could not distinguish a
clean frame from a full-grid copy. `tests/m6.rs` and
`CountersSnapshot::snapshot_counters()` now report:

- `cells_materialized` / `rows_materialized` — cells and rows copied out of
  Ghostty render state. A cursor-only frame must materialize **zero** of both;
  a one-line update materializes only that row; a full redraw materializes all
  visible rows. `dirty_rows_visited` is retained for comparison but is not a
  cost measure.
- `row_cache_copies` — times the GPUI element had to copy the Vec of shared row
  handles because a previous frame still held it. Deep copies of shaped rows
  are gone; each `CachedRow` is behind `Arc` and dirty rows are replaced in
  place. Clean/blink frames copy zero row contents.
- `mouse_events_offered` / `mouse_events_encoded` / `mouse_bytes_queued` — one
  session operation decides routing under the terminal lock. An owned event is
  reported even when Ghostty returns zero bytes (deduplicated motion), so
  `events_encoded` is not inferred from a byte count. The production queue
  receives the encoded `Vec<u8>` by move, never by clone.
- `ghostty_geometry_updates` / `pty_grid_resize_requests` — independent
  counters. Font/DPI changes increment only the former; a one-cell grid change
  increments both. Layout-originated geometry never schedules a redundant
  renderer wake.
- `scroll_lock_acquisitions` — one per scroll gesture; the thumb drag reads
  the resulting scrollbar state from the same operation.

The measured Windows and Linux/WSLg release numbers are recorded in the table
at the end of this section; the invariants above are asserted as counts in
`tests/m6.rs`, not as wall-clock thresholds.

Measured release counters (`cargo test --release --test m6 -- --nocapture`),
80x24 synthetic-fixture session. On Windows `--features shell` is omitted for
this headless test because GPUI's Windows build script needs `fxc.exe` from the
Windows SDK, which this host does not have; M6 touches only `TerminalSession`
and `RowCache`, so the extra feature is unnecessary. All numbers are from
`TerminalSession::snapshot_counters()`:

| Platform | Workload | Writes | Bytes | Wakeups sched / coalesced | Render updates | Dirty full / partial / clean | Cells materialized | Rows materialized | Rows rebuilt | Row-cache copies | Mouse offered / encoded / bytes | Ghostty geom updates | PTY grid reqs | Scroll locks | Max lock us |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Windows | Output flood | 8,000 | 216,000 | 1 / 7,999 | 57 | 57 / 0 / 0 | 109,440 | 1,368 | 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 5,797 |
| Windows | Cursor-only frame | 2 | 15 | 1 / 1 | 2 | 1 / 0 / 1 | 1,920 | 24 | 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 310 |
| Windows | Resize storm | 0 | 0 | 0 / 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 0 | 0 / 0 / 0 | 10 | 9 | 0 | 0 |
| Linux (WSL ext4) | Output flood | 8,000 | 216,000 | 1 / 7,999 | 118 | 118 / 0 / 0 | 226,560 | 2,832 | 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 572 |
| Linux (WSL ext4) | Cursor-only frame | 2 | 15 | 1 / 1 | 2 | 1 / 0 / 1 | 1,920 | 24 | 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 224 |
| Linux (WSL ext4) | Resize storm | 0 | 0 | 0 / 0 | 0 | 0 / 0 / 0 | 0 | 0 | 0 | 0 | 0 / 0 / 0 | 10 | 9 | 0 | 0 |

Linux renders more frames during the same flood (118 vs 57) because its
producer thread is scheduled differently relative to the renderer, but wakeup
coalescing (one wakeup for 8,000 writes), zero row rebuilds and zero row-cache
copies hold on both platforms. The resize storm is identical: one pixel-only
Ghostty update plus nine grid changes, nine PTY requests and 990 duplicates
suppressed, with no renderer wake scheduled from layout.

What these prove on the repair pass:

- Output flood: 8,000 writes schedule **one** UI wakeup; 7,999 are coalesced.
  Only 1,368 rows are materialized across 57 frames (24 visible rows per full
  frame), and `rows_rebuilt` / `row_cache_copies` are **zero** — the row cache
  reuses shaped rows in place and no whole-frame copy happens.
- Cursor-only frame: `dirty_clean`, zero dirty rows, zero row rebuilds and zero
  cache copies even though 24 rows were materialized for the preceding full
  frame; the cursor blink frame itself reshapes nothing.
- Resize storm: 1,000 bounds changes with nine distinct grids produce ten
  Ghostty geometry updates (one pixel-only + nine grid) and nine PTY grid
  requests; 990 duplicates are suppressed and no renderer wake is scheduled
  from layout.

Scrollback is bounded deliberately in `SessionConfig` (10,000 lines / 16 MiB by
default, override with `--scrollback-lines` and `--scrollback-bytes`). Ghostty's
own 10 KiB default byte cap is replaced so ordinary command output retains
usable history. Idle scrollback compression is **not** enabled: the profiled
workloads show no foreground stall that would justify its serialization cost.

## 17. Startup

Cold start should do only:

- initialize GPUI/Silvestris;
- register bundled fonts;
- load minimal preference/config;
- initialize Ghostty VT library/wrappers;
- create one window/session;
- spawn shell.

No plugin scans, profile discovery, font-directory crawl controlled by us, shell-integration install, network requests or session restoration.

If startup is slow, profile before caching everything. Wintty's perceived app slowness is precisely why this project should keep the host surface small.

## 18. Build optimization

Silvestris currently configures release `opt-level=3`, thin LTO and `codegen-units=1`; see [Silvestris `Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml). Keep the consumer's release profile compatible unless measurements justify changes.

Development performance tests should include an optimized build; parser/shaping behavior in debug builds is not representative.

## 19. FFI overhead

Do not assume C FFI itself is expensive; same-process calls are normally cheap. The risk is **call count**. Prefer row-level/batched APIs when they also simplify code. Ghostty's render API explicitly provides multi-get operations and row-level selection to reduce crossings when needed. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Profile before designing a bulk mirror of the entire screen.

## 20. Performance acceptance gates

### M1 renderer

- one element;
- adjacent text batching;
- clean rerender does no row rebuild.

### Live PTY

- UI thread never blocks on PTY;
- wakeups coalesce under output flood;
- max terminal lock remains bounded enough for interaction.

### Resize

- duplicate/pixel-only PTY resizes suppressed;
- live drag remains responsive.

### Long session

- memory follows configured cap;
- no increasing per-frame cost proportional to historical scrollback;
- visible-row render cost remains stable.

## Deliverables

- Debug counters.
- Repeatable output/style/resize/long-session workloads.
- Row-level dirty caching.
- Coalesced wakeups.
- Resize suppression.
- Cursor-only cheap frame.
- Memory/scrollback measurements.
- Performance notes for any hot-path change.

## Avoidables

- Benchmark-only architecture.
- custom parser optimization.
- custom GPU renderer.
- continuous frame loop.
- per-cell allocations.
- massive caches before evidence.
- sleeping reader to batch.
- Tokio/IOCP solely for “speed.”
- performance claims from debug builds.

## Regression traps

- seemingly harmless refactor restores full-row/full-grid shaping;
- cache invalidation bug after DPI/font change;
- wakeup batching adds noticeable key echo delay;
- performance counters themselves allocate/log on hot path;
- scrollback compression holds terminal lock during interaction;
- writer queue becomes unbounded;
- resize debounce leaves TUI at stale dimensions;
- background output keeps hidden/minimized window rendering continuously.

## Source anchors

- Ghostty: [README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h)
- Silvestris: [`Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml), [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`window.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/window.rs)
