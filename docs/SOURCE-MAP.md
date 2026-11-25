# Source Map and Revision Pins

This packet is intentionally source-grounded. The URLs below are immutable commit links so later upstream changes do not silently change the evidence behind an engineering decision. Before implementing the project, verify whether newer released versions materially change any API, then update the pin **deliberately** rather than replacing links with `main`.

## Implemented pins (M0) — 2026-11-24

| Dependency | What pain consumes | Pin |
|---|---|---|
| `libghostty-vt` | vendored source snapshot in `vendor/ghostty-vt` | release tag `vt-1.0.2` / commit `573c4a21e02311ae87844af26b6a7a179f82858e` |
| Silvestris | path dependency on the freshest `~/workdir/silvestris-cont` checkout, crate `crates/silvestris` | branch `devel/sil-cocoa-fixture`, HEAD `30173a5860ad263feacb3d417a73c3a354a21e78` when wired; the path follows future HEADs |
| Zed/GPUI | exact revision inherited from `silvestris-cont/config/pins.toml` | `6b720973027999bae402df23d469026f4cced988` (unchanged) |
| Rust | `silvestris-cont/rust-toolchain.toml` | `1.98.1`, GNU host on Windows |

`vt-1.0.2` is the released tag closest to the research snapshot: the research
commit `64a6fedfd26d1fa687733f9f411db5eb8e205ebe` is `bump for lib-vt 1.0.3-dev`
made 31 minutes after the `vt-1.0.2` tag, so the API studied in the packet is
the 1.0.2 API. Pain does not track `main` and does not use the research commit
as a dependency. `vendor/ghostty-vt/PIN.txt` records the exact source snapshot;
`crates/ghostty-vt-sys/build.rs` refuses to build if that pin changes.

The Silvestris dependency is optional (`shell` feature) so M0's headless tests
do not compile GPUI. It is deliberately a path dependency to the freshest
`silvestris-cont` checkout, as requested; keep that checkout clean and update
the recorded HEAD here whenever it moves. The GPUI git revision in
`pain/Cargo.toml` is pinned to the same rev as that checkout, so enabling the
feature cannot introduce a second GPUI type universe.

### Build and test commands

Zig 0.17.x is required (0.17.1 verified). From a Git snapshot `-Demit-lib-vt`
is mandatory; release tarballs carry `LIB_VERSION` and do not need it. The
Rust build script runs Zig automatically, installs into Cargo's `OUT_DIR`,
and emits the static link flags.

```sh
# Windows (PowerShell), from pain/
cargo test --workspace --all-targets

# Linux (bash), from pain/
cargo test --workspace --all-targets

# Direct library build, if a C consumer needs the headers/libs:
zig build --build-file vendor/ghostty-vt/build.zig \
  --prefix <out-dir> -Demit-lib-vt -Doptimize=ReleaseFast -Dcpu=baseline
```

Windows installs `lib/ghostty-vt-static.lib` (plus `bin/ghostty-vt.dll` and
`lib/ghostty-vt.lib` for the shared build); Linux installs
`lib/libghostty-vt.a` and `lib/libghostty-vt.so*`. The static link also needs
`ntdll`/`kernel32` on Windows and `pthread`/`m`/`dl` on Linux. Zig emits
`call __chkstk; sub %rax, %rsp`, i.e. a probe that must not move RSP and must
preserve RAX and the argument registers. MinGW libgcc's `___chkstk` moves RSP
and `___chkstk_ms` clobbers RCX, so Windows links the small local probe in
`crates/ghostty-vt-sys/chkstk.S` instead. Set `ZIG` to choose a different
compiler or `GHOSTTY_VT_SOURCE` to point at another pinned source tree (its
`PIN.txt` must still match).

Cross-building the library from Windows for Linux is possible with
`-Dtarget=x86_64-linux-gnu`; the static `libghostty-vt.a` installs correctly.
On Windows-hosted filesystems Zig cannot create the Linux shared-library
symlinks (`PermissionDenied`), so that last install step fails there; a real
Linux host creates the symlinks normally. The Rust static link does not need
them.

The FFI declarations in `crates/ghostty-vt-sys/src/lib.rs` are handwritten
against the 1.0.2 headers; unit tests assert their `size_of` values against
sizes measured from those headers with `zig cc`. Keep that test green when
touching the declarations.

Verification command for both platforms:

```sh
cargo test --workspace --all-targets
```

This passed on both Windows (GNU host) and Linux (Fedora via WSL): 1 ABI test
plus 7 headless tests, all green on each. The repeated-create/free loop
creates, feeds, renders, inspects and drops 256 terminals. No window or PTY is
opened in either run.

## Implemented milestones M1/M2 — 2026-11-24

M1 and M2 add no new upstream pins. The M1 window renderer consumes the same
Silvestris checkout and the same GPUI revision listed above; the local path
dependency still resolves to `~/workdir/silvestris-cont/crates/silvestris`.
`gpui_platform` is also taken from the pinned Zed revision so one GPUI type
universe is used end to end. `async-channel` is only the capacity-one wakeup
bridge to GPUI's foreground executor; it adds no separate runtime.

### M1/M2 build and test commands

```sh
# Windows (PowerShell), from pain/
cargo test --workspace --all-targets                 # headless M0/M1/M2 logic
cargo test --workspace --all-targets --features shell
cargo run --features shell                           # one window, canned fixture
$env:PAIN_STRESS = 1; cargo run --features shell     # synthetic M2 producer

# Linux (bash), from pain/
cargo test --workspace --all-targets
cargo test --workspace --all-targets --features shell
cargo run --features shell
PAIN_STRESS=1 cargo run --features shell

# Linux under WSLg, from the Windows host:
wsl -d FedoraLinux-44 -- bash -lc 'cd /mnt/c/Users/terezi/workdir/ghostty/pain \
  && export ZIG=/home/terezi/zig-linux/zig CARGO_BUILD_BUILD_DIR=/home/terezi/cargo-build \
     DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 \
  && cargo run --features shell'

# Preferred: keep the working copy and target dir on WSL ext4, not /mnt/c.
# The relative Silvestris path dependency needs the /workdir/<repo> layout.
cp -a --reflink=auto /mnt/c/Users/terezi/workdir/ghostty/pain /home/terezi/workdir/ghostty/
cp -a /mnt/c/Users/terezi/workdir/silvestris-cont /home/terezi/workdir/
cd /home/terezi/workdir/ghostty/pain
export ZIG=/home/terezi/zig-linux/zig CARGO_TARGET_DIR=/home/terezi/pain-target
export DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000
cargo test --workspace --all-targets --features shell
cargo run --features shell
```

On this Windows host the MSYS2 `windres` from `PATH` mangles the backslash in
GPUI's `resources/windows\gpui.rc` when `gpui_platform` enables
`gpui/windows-manifest`. A local shim directory containing a `windres.cmd`
that converts `\` to `/` and is prepended to `PATH` lets Cargo build GPUI
unchanged; no upstream or Silvestris file is modified. Linux needs no shim.

### What M1/M2 verify

- Snapshot/logical tests: row batching, wide-grapheme tails, background-span
  coalescing, cell-to-pixel geometry and clean-frame row-cache reuse in
  `src/terminal/geometry.rs`.
- M1 acceptance: canned ANSI/256/true-color, wide-character, underline/bold
  and cursor fixtures render through one `TerminalElement`; unchanged frames
  reuse cached rows (no per-frame full-grid rebuild).
- M2 acceptance: headless stress tests show 2 000 VT writes producing one
  scheduled wakeup, re-arming after a consumed wakeup, successful concurrent
  render while a producer writes, and terminal lock hold time bounded to VT
  processing. `TerminalSession::snapshot_counters` exposes the
  docs/PERFORMANCE.md counters.

Verified green on both Windows (GNU host) and Linux (WSL Fedora): 1 ABI test +
31 unit tests + 8 M0 integration tests + 6 M1/M2 integration tests + 4 Windows
PTY integration tests (cmd, PowerShell, vim/mouse mode, counters) and 2 Linux
PTY integration tests, with and without `--features shell`. A WSLg window opens
via `cargo run --features shell`; interactive typing, Enter, Backspace and
font resizing were confirmed working by the user in WSLg and native Windows.
`PAIN_FIXTURE=1` still shows the canned
ANSI/bold/underline/wide-character fixture. CJK glyphs render as tofu boxes in
that Linux container because it has no CJK font installed; Ghostty's grapheme
and two-cell geometry remain correct.

### What M3/M4 verify

- `tests/pty.rs` spawns a real shell on each platform headlessly, writes a
  command into the ordered writer queue, reads the parsed output through
  `TerminalSession::render_snapshot`, resizes the grid (including a duplicate
  that the backend coalesces) and shuts down with the child reaped. Windows
  uses ConPTY with a named input pipe and `cmd.exe`; Linux uses `openpty` with
  `/bin/sh`.
- Pure helpers are unit-tested: Windows command-line quoting and Unicode
  environment blocks, POSIX winsize conversion and `$SHELL` fallback, GPUI
  key/modifier/mouse mapping, and writer-queue ordering/blocking shutdown.
- ConPTY details that matter: `STARTUPINFOEXW.cb` is the extended struct size,
  `lpValue` for `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` is the HPCON value itself
  (as in Microsoft's MiniTerm sample), `CREATE_UNICODE_ENVIRONMENT` is required
  for a UTF-16 environment block, and `STARTF_USESTDHANDLES` must be set even
  though ConPTY replaces the child's std handles. The pipe ends handed to
  `CreatePseudoConsole` stay owned for the session.
- The app launches the real shell by default; use `--shell <program> [args]` or
  `-- <program> [args]` to override, `PAIN_FIXTURE=1` for the canned snapshot,
  and `PAIN_STRESS=1` for the M2 producer. `Ctrl+Shift+C/V/A` are the only
  clipboard/selection commands sent to the app; every other key goes through
  Ghostty's encoder to the PTY.

`TerminalElement` passes one cell width (not the run width) as `shape_line`'s
`force_width`: GPUI treats that argument as the per-glyph advance, so passing
a run width spreads glyphs apart.

Font-size commands are whitelisted global actions (`Ctrl+=`, `Ctrl+-`,
`Ctrl+0`) that only change `TerminalView`'s font-size override; the family
still comes from the active Silvestris theme. Window resize handling derives
grid size from real cell metrics, calls `ghostty_terminal_resize` only when
rows/columns change, and reuses cached rows for pixel-only size changes.

## Research baseline — 2026-11-24

| Project | Branch/context | Revision |
|---|---|---|
| Ghostty | `main` research snapshot | `64a6fedfd26d1fa687733f9f411db5eb8e205ebe` |
| Wintty | `windows` branch research snapshot | `047cd33cd1a24602cc4887ac260e2f922943cac0` |
| Silvestris | `devel/sil-cocoa-fixture` research snapshot | `0d5cc609171992d003994c8d8fd44b014b4d0b94` |
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
