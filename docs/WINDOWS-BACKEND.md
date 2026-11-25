# Windows Backend

The Windows backend provides **transport and child-process ownership only**. It creates one OS ConPTY, launches one child process attached to that pseudoconsole, moves bytes between ConPTY and the shared `TerminalSession`, resizes the pseudoconsole when the terminal grid changes, and shuts the process down cleanly. It does not render terminal content, interpret ANSI, own keyboard focus, or create a second native child window.

This is deliberately smaller than Wintty. Current Wintty uses Ghostty's larger Windows integration, WinUI 3 application shell and a DirectX 12 terminal renderer; its PTY layer is still valuable as a source reference because it confirms ConPTY is its sole Windows transport and shows the exact create/resize/close seam. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig). Ghostty's VT library remains the terminal engine and explicitly leaves PTY transport to us. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). The visible HWND/window remains GPUI/Silvestris-owned; current Silvestris already uses GPUI/native frame integration for Windows chrome. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs) and [GPUI platform source](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 1. MVP policy

Use the **OS-provided ConPTY implementation**. Do not bundle a private `conpty.dll` in the first release.

Wintty's current PTY source optionally looks for an adjacent newer `conpty.dll` and falls back to the kernel32 ConPTY API. Its comments explain why: the in-box path can strip Kitty graphics APC and Sixel DCS sequences, while a newer OpenConsole-based DLL can pass them through. Those protocols are explicitly out of scope for this minimal terminal, so carrying a private console host would add build, signing, servicing and compatibility complexity without improving the feature set we actually render. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig).

If later work adds image protocols and proves the OS host is the blocker, revisit this as a separate milestone.

## 2. Backend shape

Recommended internal types:

```rust
struct WindowsPty {
    hpcon: HPCON,
    input_write: OwnedHandle,
    output_read: OwnedHandle,
    process: OwnedHandle,
    process_id: u32,
    writer_tx: Sender<WriteCommand>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    waiter: Option<JoinHandle<()>>,
}
```

Exact Rust ownership may differ, but every native handle must have one clear owner. The terminal/common layer should see only a safe `PlatformPty` interface.

Use the `windows` crate already present in/compatible with the Silvestris ecosystem rather than a custom C shim unless a missing binding is proven. Add only the Win32 feature modules needed for ConPTY/process/pipe operations; avoid broad feature activation.

## 3. ConPTY creation

Conceptual sequence:

1. determine initial rows/columns from terminal geometry;
2. create input and output pipe pairs;
3. pass the child-facing pipe ends to `CreatePseudoConsole`;
4. retain host-facing input-write and output-read ends;
5. construct a `STARTUPINFOEXW` attribute list;
6. set `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` to the created `HPCON`;
7. create the child process with extended startup info;
8. close host copies of pipe ends that are only needed by ConPTY/child;
9. start reader/writer/waiter workers.

Wintty's PTY source calls `CreatePseudoConsole` with the cell size and the PTY-facing input/output pipe handles, then stores the returned `HPCON`; its resize path calls `ResizePseudoConsole` with a new `COORD`, and deinit calls `ClosePseudoConsole`. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig).

Do not expose `HPCON` outside `platform/windows.rs`.

## 4. Pipe strategy

For a **single-session** terminal, correctness and clean shutdown matter more than reproducing Wintty's high-scale I/O implementation. A blocking reader thread on the output pipe and a blocking writer thread on the input pipe are acceptable and easy to audit. They keep all blocking Win32 I/O off GPUI's foreground thread.

Wintty uses an overlapped named pipe for one direction because its IOCP/libxev backend requires overlapped file operations; its source explicitly comments that anonymous `CreatePipe` handles do not support that model. See [Wintty PTY source](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig). We do not need to inherit that machinery merely to support one terminal.

However, choose pipe creation so shutdown can reliably unblock a blocked read/write. If simple synchronous anonymous pipes make teardown unreliable in practice, move to overlapped named pipes as a measured correction rather than an up-front framework.

## 5. Process creation

The backend should launch a command line supplied by higher-level policy. The platform layer owns Windows quoting/UTF-16 conversion and process attributes; it does not decide terminal preferences.

For MVP shell policy, support:

- explicit `--shell` / configured command when present;
- otherwise a conservative Windows default chosen by the app, documented clearly.

Do not silently probe and switch among PowerShell 7, Windows PowerShell, cmd, WSL, Git Bash and MSYS shells. That turns shell discovery into a product subsystem. A simple explicit default plus command override is sufficient.

The child inherits the user's environment with only terminal-specific additions/overrides such as `TERM`/`COLORTERM`. Do not replace PATH or construct a cleaned environment unless necessary.

## 6. Terminal environment

Do not set `TERM=xterm-ghostty` initially. This program is not Ghostty's full renderer and intentionally omits graphics and other protocols. Start with a conservative identity such as:

```text
TERM=xterm-256color
COLORTERM=truecolor
```

Verify real TUI behavior before changing it.

ConPTY itself translates between Windows console applications and VT streams. Do not insert an additional ANSI converter.

## 7. Reader thread

The reader owns `output_read` and loops:

```text
ReadFile(output_read, buffer)
    -> 0/EOF: child/pty ending
    -> bytes:
         lock GhosttyTerminal
         ghostty_terminal_vt_write(bytes)
         unlock
         generation++
         coalesced GPUI wake
```

Use a reusable buffer. Do not allocate a new Vec for every small read unless the OS API requires ownership transfer. The parser should consume the bytes before the buffer is reused.

Ghostty's terminal is the parser; do not normalize CR/LF, strip escape sequences or decode UTF-8 in the transport. See [Ghostty terminal API](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Errors during intentional shutdown should be distinguished from unexpected read failures. Do not display a scary terminal error after handles were deliberately closed.

## 8. Writer thread

Writer queue entries are ordered byte buffers plus shutdown command:

```rust
enum WriteCommand {
    Bytes(Arc<[u8]>),
    Close,
}
```

The writer thread calls `WriteFile` until each entire buffer is consumed. Handle partial writes. Never assume one `WriteFile` consumes the entire buffer.

Sources include:

- Ghostty-encoded key events;
- committed text;
- Ghostty paste encoding;
- Ghostty `WRITE_PTY` terminal replies;
- Ghostty mouse encoding.

The writer queue is the one ordering point. Do not create separate “user input” and “terminal reply” writers that can race.

## 9. Terminal protocol replies

Ghostty can invoke `WRITE_PTY` while processing child output. That callback runs under the terminal-processing serialization context. It must copy/own the returned bytes as required by the C API and enqueue them to the writer without blocking on `WriteFile`.

This avoids lock inversion:

```text
BAD:
terminal lock -> callback -> WriteFile blocks -> child blocked waiting UI -> deadlock

GOOD:
terminal lock -> callback -> queue bytes -> return/unlock
writer thread -> WriteFile
```

See [Ghostty terminal callback/options API](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 10. Process wait

Use a dedicated lightweight wait path—typically a thread waiting on the process handle. When the child exits:

- capture exit code;
- signal `TerminalSession` lifecycle;
- close/stop writer input;
- allow ConPTY/output to drain briefly if needed;
- wake GPUI so UI can close or display process-exited state.

Do not poll the process handle every animation frame.

## 11. Resize

When the GPUI terminal element computes a new **cell grid**, send one backend resize command:

```text
ResizePseudoConsole(hpcon, COORD { X: cols, Y: rows })
```

Wintty does exactly this in its current PTY source. See [Wintty `src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig).

Resize should be coalesced at two levels:

- `TerminalElement` ignores pixel changes that do not change rows/cols/cell metrics;
- the Windows backend ignores a requested `COORD` equal to its last successful size.

Zed's pinned terminal has a regression test for avoiding PTY resize on pixel-only changes; use the same principle. See [Zed `terminal.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/terminal/src/terminal.rs).

Call `ghostty_terminal_resize` in the shared session too, with rows/cols and cell pixel dimensions. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 12. Native window resize loops

Wintty's standalone Win32 example starts an 8 ms timer during `WM_ENTERSIZEMOVE` because a raw Win32 host can enter a modal move/resize loop and must keep ticking Ghostty's renderer. See [Wintty Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c).

Do **not** copy that timer into this app automatically. GPUI owns the HWND/message loop and frame scheduling. First verify how the pinned GPUI Windows backend behaves during live resize. If it already continues frames, adding a second timer creates redundant wakeups. Only add a workaround in the GPUI/Silvestris integration if profiling demonstrates an actual stall.

## 13. Keyboard and text

Do not consume raw Win32 `WM_KEYDOWN`/`WM_CHAR` directly. GPUI owns the native window and translates platform input. The terminal receives:

- GPUI key events for physical/non-text keys;
- GPUI `InputHandler` callbacks for committed/composed text.

Wintty's Win32 example manually handles `WM_UNICHAR`, UTF-16 surrogate pairs, key repeat, focus, wheel and DPI because it owns the native window. Use that source as an **edge-case checklist**, not an event source. See [Wintty Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c). GPUI's platform input trait is the correct boundary here. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 14. Ctrl+C / Ctrl+Break

The GUI process must not accidentally treat Ctrl+C as a console-control signal intended for itself. Whether the GUI is attached to a parent console depends on packaging/launch. Do not allocate/attach a debug console in release solely for logs.

Terminal Ctrl+C should be encoded as terminal input by Ghostty key handling and written to ConPTY. Test:

- shell prompt interrupt;
- running process interrupt;
- copying selected text if the app chooses Ctrl+C as copy only under selection;
- no GUI termination.

Wintty's raw example installs a console ctrl handler because it intentionally attaches/allocates a console for diagnostic output. Our normal GUI app should avoid needing that pattern. See [Wintty Win32 example](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c).

## 15. Focus

Focus remains GPUI/Silvestris-owned. When the GPUI terminal view gains/loses focus, update any Ghostty terminal focus-reporting protocol state as needed and repaint cursor style. Do not call `SetFocus` on a separate terminal HWND because there is no child terminal HWND.

Silvestris already manages native window frame/focus integration; see [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs) and [`native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs).

## 16. DPI

GPUI handles window DPI/content scale. Terminal code measures GPUI font/cell geometry, then passes integer cell pixel dimensions to Ghostty terminal resize. The Windows backend only needs rows/columns for ConPTY.

Do not independently react to `WM_DPICHANGED` in the PTY backend.

## 17. Clipboard

Clipboard is an app/GPUI concern. Windows backend does not call Win32 clipboard APIs directly unless GPUI lacks a required capability. User paste goes GPUI clipboard → Ghostty paste API → writer queue. Copy goes Ghostty selection formatter → GPUI clipboard.

Program-initiated OSC clipboard access remains disabled initially.

## 18. Shutdown

The exact close sequence must be tested because ConPTY/pipe close ordering can affect EOF.

Recommended intent:

1. terminal session enters `Closing`;
2. stop accepting new key/paste writes;
3. tell writer to close;
4. close host input-write end;
5. close pseudoconsole / terminate child only according to chosen graceful-close policy;
6. ensure output read unblocks;
7. wait/reap child;
8. close remaining process/pipe handles;
9. join workers off foreground thread;
10. mark session `Closed`.

Do not call `TerminateProcess` as the normal close mechanism without first defining expected shell-close semantics. Closing the pseudoconsole/pipe should normally signal terminal loss; add forced termination only as bounded fallback.

## 19. Error mapping

Map Win32 errors into concise domain errors with operation context:

```text
CreatePseudoConsole failed: HRESULT ...
CreateProcessW failed: ...
terminal output ReadFile failed: ...
ResizePseudoConsole failed: ...
```

Store native error code for diagnostics. Do not stringify/lose it at the FFI boundary.

An isolated resize failure should not necessarily destroy the session if the old size remains valid; log and retry only on a later distinct size. Creation/read/write failures are session-fatal.

## 20. Windows tests

### Pure/unit

- command-line quoting;
- UTF-16 conversion with embedded invalid/null rejection;
- COORD clamping;
- resize coalescing;
- handle-ownership state machine;
- writer partial-write loop using test abstraction if practical.

### Integration

- spawn `cmd.exe`, print marker, exit code;
- spawn PowerShell, Unicode output/input;
- Ctrl+C behavior;
- alternate-screen TUI;
- rapid resize;
- large output while dragging window;
- child exit while writer queue nonempty;
- close while reader blocked;
- repeated create/close to detect leaked handles/processes.

### Manual

- IME/dead keys;
- clipboard;
- Snap/maximize/restore via Silvestris titlebar;
- different DPI displays;
- WSL command if available;
- long terminal-agent session.

## Deliverables

- OS ConPTY backend with one child process.
- Blocking I/O fully off GPUI foreground thread.
- One ordered writer queue.
- Process wait/exit propagation.
- Cell-based resize coalescing.
- Clean handle ownership and teardown.
- Windows key/text behavior driven by GPUI, not raw HWND interception.
- Real-shell integration tests.

## Avoidables

- WinUI 3.
- Wintty application dependency.
- Ghostty DX12 surface.
- private `conpty.dll`.
- IOCP/libxev machinery before need.
- extra native child HWND.
- Win32 clipboard path if GPUI already suffices.
- direct Win32 key/message interception.
- shell auto-discovery matrix.

## Regression traps

- inherited handles preventing EOF;
- reader/write thread blocked forever during close;
- GUI process receiving Ctrl+C control event;
- duplicate key/text emission;
- stale resize after newer resize;
- every pixel resize becoming `ResizePseudoConsole`;
- process exit not reaped;
- ConPTY bytes decoded/modified before Ghostty;
- output thread touching GPUI directly;
- bundling richer ConPTY later without matching graphics renderer;
- foreground thread waiting on worker joins.

## Source anchors

- Ghostty: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
- Wintty: [`src/pty.zig`](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/src/pty.zig), [minimal Win32 host](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c)
- Silvestris: [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs), [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md)
- GPUI: [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs), [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`window.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/window.rs)
