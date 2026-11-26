# GPUI Rendering and Input

GPUI is the only GUI renderer in the initial terminal. Ghostty provides terminal state, not pixels; platform PTYs provide bytes, not widgets; Silvestris composes the application window. The terminal therefore uses GPUI at its lowest appropriate public level: one custom `Element` for the terminal canvas, GPUI's text system for shaping, paint primitives for cells/cursor/selection, `FocusHandle` for keyboard ownership, and `InputHandler` for platform-native text/IME.

At the exact GPUI commit pinned by the current Silvestris branch (`6b720973027999bae402df23d469026f4cced988`), the `Element` trait is explicitly documented as the contract for types that participate in window layout/painting and as the route for custom elements. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs). Silvestris requires consumers to use that exact pin. See [Silvestris consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md). Ghostty's VT API deliberately leaves drawing to the embedder. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 1. Why a custom element

A terminal is a dense, uniform, continuously updating grid. Representing each cell as a GPUI `div`/text child would force thousands of layout nodes, event/hit-test objects and allocations for data that is naturally painted in batches.

Instead:

```text
TerminalView::render()
    └─ TerminalElement
        request_layout()
        prepaint()
        paint()
```

The element occupies one rectangular content region and internally turns Ghostty's dirty rows into GPUI text/quads.

Zed's terminal at the same pinned revision follows this broad pattern and explicitly batches adjacent equal-style cells in `BatchedTextRun`. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs). We should borrow the architecture, not depend on Zed terminal crates.

## 2. `Element` lifecycle

The pinned GPUI source documents:

- `request_layout`: request a Taffy layout node and establish layout-phase state;
- `prepaint`: after bounds are known, commit hitboxes and prepare paint state;
- `paint`: draw to the current frame using the bounds/state from prior phases.

See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

### `request_layout`

Request a full available content rectangle with any minimum size required by the app. Do not inspect terminal rows here if bounds are not final.

Layout state can contain lightweight font/config identity if necessary, but avoid terminal locking.

### `prepaint`

This is the main preparation phase:

1. read final bounds;
2. resolve/measure terminal font/cell metrics;
3. derive grid rows/columns;
4. request a session resize if terminal geometry changed;
5. synchronize Ghostty render state;
6. rebuild dirty row render caches;
7. register terminal hitbox/focus/input handler;
8. prepare cursor/selection/quads for paint.

Do not perform blocking PTY I/O. The only terminal lock section should be Ghostty render-state begin/update or short terminal queries.

### `paint`

Paint prepared data only:

1. base terminal background;
2. per-cell/background runs;
3. selection backgrounds;
4. shaped text;
5. decorations not encoded in text runs;
6. cursor;
7. IME composition indicator if needed.

Avoid allocation and terminal locking here where practical.

## 3. Text system

GPUI's `TextRun` represents a styled run, and `TextSystem::shape_line` shapes a single line of text. See [GPUI `text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs). The terminal should turn compatible adjacent Ghostty cells into a small set of `TextRun`/batched strings per terminal row.

Do not call `shape_line` once per cell. Zed's pinned terminal is useful evidence: it has a terminal-specific batch type precisely to combine adjacent same-style cells before shaping. See [Zed `terminal_element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal_view/src/terminal_element.rs).

### Cache policy

Cache by terminal row and render-relevant version. A row cache becomes invalid when:

- Ghostty marks row dirty;
- full terminal dirty state is set;
- grid geometry/reflow changes;
- font family/size/features change;
- DPI/content scale changes;
- global terminal palette/default colors change in a way not reflected as row dirty;
- renderer implementation/version changes.

Cursor blink alone must not invalidate shaped row text.

### Font metrics

Measure the actual resolved monospace font. Terminal cell width must be deterministic. If the chosen font has a representative mono advance, use GPUI measurement rather than a hardcoded width. Row height should be the font's own line box (`ascent + descent`, clamped to at least the font size): GPUI centers each shaped row inside the height passed to `ShapedLine::paint`, so extra leading becomes a visible gap between rows and breaks contiguous box-drawing borders in TUIs.

The application can default to Silvestris's mono role, but GPUI owns actual font resolution/shaping. See [Silvestris fonts/theme guidance](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

## 4. Positioning text in a cell grid

Shaped text must be anchored to terminal columns, not laid out as a flowing paragraph. For each batched run:

```text
x = content_left + start_col * cell_width
y = content_top  + row * line_height + baseline_adjustment
```

The run may contain multiple cells, but the next batch's x position still comes from terminal column, not the previous shaped line's natural width.

Wide glyphs/grapheme clusters are taken from Ghostty's cell data. Do not advance a column based on Unicode scalar count.

If a fallback font produces unusual advance, terminal geometry remains authoritative. Clamp/position rather than allowing cumulative drift across the line.

## 5. Backgrounds and quads

Solid terminal backgrounds, selection and block cursor are natural GPUI quads. The pinned GPUI codebase exposes `window.paint_quad` and uses it throughout custom painting. See [GPUI `window.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/window.rs).

Coalesce adjacent equal-color background cells into horizontal runs:

```text
[black][black][black][red][red][black]
 -> black run 0..2
 -> red run   3..4
 -> black run 5
```

Skip background runs equal to the terminal's base background unless selection/cursor requires an override.

Selection should normally be a separate overlay/background layer so it does not split every text run unnecessarily.

## 6. Underline, strike and terminal decorations

Use GPUI text-run decoration support when it maps exactly and cheaply. If terminal underline style/color semantics exceed GPUI's run model, paint the decoration as geometry after text/background positioning. Keep terminal decoration logic inside the renderer, not the Ghostty wrapper.

Test single/double/curly underline only when the selected Ghostty API exposes those styles and MVP needs them. Basic underline/bold/italic is enough for the first correctness milestone; unsupported exotic decoration must degrade visibly but not corrupt grid geometry.

## 7. Cursor

Ghostty render state exposes cursor visibility, viewport position, visual style, blink state and related information. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h).

Paint cursor according to style:

- block: full-cell fill or inverse treatment;
- bar: thin vertical quad;
- underline: thin bottom quad;
- hollow block: stroked rectangle if supported cheaply.

Cursor blink is a GPUI repaint concern. Store the last cursor data in `TerminalView`; a blink timer toggles visibility and invalidates the terminal view without refreshing/shaping the grid. If the terminal reports non-blinking/hidden cursor, stop the timer.

## 8. Clipping

Clip all terminal painting to the element bounds. A partially visible last row/column must not draw into Silvestris chrome or resize edges. The terminal hitbox should likewise match only the content rectangle, not the titlebar/client-decoration regions.

This is particularly important on Linux because Silvestris uses client-side window decorations. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

## 9. Focus

Create one `FocusHandle` for `TerminalView` and make the terminal surface focusable. GPUI describes focus handles as the mechanism to track/manipulate keyboard focus. See [GPUI `app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs).

Behavior:

- terminal receives initial focus when the window opens;
- mouse-down inside content focuses terminal before processing input;
- opening a context menu may move focus temporarily, then Silvestris focus-return machinery restores terminal;
- inactive/unfocused terminal cursor can render hollow/dim according to desired legacy style;
- terminal does not use a hidden `TextField` solely to capture keyboard focus.

Silvestris responder capture should be layered around the same focus handle. See [Silvestris `responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs).

## 10. Key events vs text input

GPUI/platform key events give physical/logical keystroke information. Terminal key encoding needs those for navigation/function/control combinations. Unicode committed text needs the platform text-input path.

GPUI's `InputHandler` is the bridge from platform-native text protocols; its source notes offsets/ranges are UTF-16 unless otherwise specified. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs). Implement the minimal handler required for:

- text insertion/commit;
- marked/composition text if GPUI requires it;
- selected-range/query methods expected by platform IME;
- bounds/range information sufficient for candidate-window positioning.

Do not claim full editor semantics; the terminal does not have a normal editable document. The handler can present a small logical composition buffer while committed text is sent to the terminal.

## 11. IME

IME candidate placement should be near the terminal cursor. Use the cached cursor cell and terminal bounds to answer the platform's text bounds query. When composition text is uncommitted, optionally paint it at/near the cursor using a small overlay; when committed, send UTF-8 text through terminal input/PTY writer.

Regression tests should include:

- dead keys;
- accented Latin text;
- non-BMP character/surrogate case on Windows;
- CJK IME if available;
- composition cancelled by focus loss.

Wintty's raw Win32 sample manually handles `WM_CHAR`, surrogate pairs and `WM_UNICHAR` because it owns the HWND. In our app, GPUI should own those platform translations; the sample remains a test checklist. See [Wintty Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c).

## 12. Mouse events

GPUI supplies pointer positions in window logical coordinates. Convert to terminal-local coordinates:

```text
local = event.position - terminal_bounds.origin
col = floor(local.x / cell_width)
row = floor(local.y / line_height)
```

Clamp only after checking whether the pointer is inside the terminal bounds. Track left/right side of the cell if Ghostty selection gestures require it.

Mouse reporting uses Ghostty's encoder once active terminal modes request it. See [Ghostty mouse example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c). Local selection remains application behavior.

During drag outside the terminal bounds, selection may auto-scroll. Implement only after basic selection is correct; avoid introducing high-frequency timers that repaint the entire grid.

## 13. Scroll events

GPUI scroll deltas may be pixel or line based depending on platform/device. Normalize to a terminal scroll policy without discarding high-resolution trackpad/wheel information. Maintain a fractional accumulator if needed, emitting Ghostty viewport row deltas only when enough motion accumulates.

If mouse reporting/alternate-scroll is active, convert scroll into terminal mouse events/keys according to Ghostty mode/encoder behavior. Do not blindly scroll local history inside full-screen TUIs.

## 14. Frame invalidation

Sources of terminal repaint:

- PTY output generation changes;
- cursor blink;
- selection drag;
- scrollback viewport movement;
- resize/font/DPI change;
- focus/active-window change;
- theme change affecting app/default terminal visual;
- IME composition.

Each source should invalidate only the view. Do not run a continuous 60/120 Hz animation loop when terminal is idle.

GPUI's window/platform code schedules frames through invalidation/request-frame paths; use entity/view notification mechanisms rather than a custom renderer loop. See [GPUI `window.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/window.rs).

## 15. Background work

PTY reader/writer/process wait happen on dedicated OS/background threads. Expensive non-UI terminal maintenance such as scrollback compression may use a background executor only if it still obeys terminal serialization.

Do not move GPUI text shaping to arbitrary threads unless GPUI explicitly documents the text system as safe for that use at the pinned revision. Keep shaping in prepaint/paint path unless measured otherwise.

## 16. Accessibility

MVP accessibility target is modest but should not be actively hostile. The terminal surface can expose a basic accessible role/name and perhaps current visible text later. Do not attempt a full virtual accessibility tree for every cell in the first version. Ensure custom hitboxes/focus do not prevent keyboard focus or context menu access.

If accessibility work requires thousands of nodes per frame, defer it until a terminal-specific strategy is designed.

## 17. Performance instrumentation at GPUI boundary

Count:

- `prepaint` calls;
- render-state updates;
- dirty rows consumed;
- row caches rebuilt;
- text batches created;
- `shape_line` calls;
- quads emitted;
- cursor-only frames;
- wakeups requested/consumed;
- frame time spent in terminal prepaint and paint.

These counters are more actionable than overall FPS because a fast GPU can hide waste until lower-end hardware or long sessions.

## 18. Reference to Zed—use carefully

Zed's terminal code at the same GPUI pin is highly relevant because it has already solved many GPUI-terminal rendering details. Use it to validate:

- custom element lifecycle;
- batched text;
- font/cell measurement;
- cursor/selection painting;
- clipping;
- input-handler integration;
- resize coalescing.

Do **not** copy Alacritty-specific models into this Ghostty integration. Ghostty render state already provides dirty rows and selection/cursor data differently. The goal is GPUI-native rendering, not a fork of Zed terminal.

## Deliverables

- One custom `TerminalElement`.
- Row-level dirty caching and adjacent text batching.
- GPUI text shaping and quad painting only.
- Focus and native text/IME integration.
- Pointer-to-cell geometry and clipping.
- Cursor-only repaint path.
- Instrumentation proving clean rows are not reshaped.

## Avoidables

- Per-cell elements.
- Continuous render loop.
- Secondary GPU renderer/surface.
- UI-thread PTY operations.
- Hard-coded cell metrics.
- Unicode `char`-based cell counting.
- Hidden generic text editor as terminal input.
- Shaping clean rows for cursor blink.
- Direct Win32/Wayland/X11 event handling in terminal renderer.

## Regression traps

- Text run cache surviving font/scale change.
- Paint coordinates using natural shaped advance rather than terminal columns.
- Candidate IME window positioned at stale cursor after scroll.
- Full terminal hitbox occluding Silvestris titlebar/resize areas.
- Focus not restored after popup.
- Printable key emitted both as key sequence and committed text.
- Scroll delta signs/units differing between Windows/Linux.
- Background quads not clipped at content edge.
- Selection style causing unnecessary run fragmentation.

## Source anchors

- Ghostty: [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [mouse example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs), [`app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs)
