# Linux Backend

The Linux backend is a thin POSIX PTY/process layer underneath the same `TerminalSession` and GPUI renderer used on Windows. It should contain no GTK code, no Wayland/X11 code, no ANSI parser, no font renderer and no separate scrollback implementation. Linux windowing remains GPUI/Silvestris's responsibility; the terminal backend only launches a child connected to a pseudoterminal and transports bytes.

Ghostty's current POSIX PTY implementation is a useful concrete checklist: it uses `openpty`, sets `FD_CLOEXEC` on the master, enables `IUTF8`, resizes with `ioctl(TIOCSWINSZ)`, resets child signal handlers, creates a new session with `setsid`, and sets the slave as controlling terminal with `TIOCSCTTY`. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig). `libghostty-vt` then owns terminal state but explicitly does not own the PTY. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). Silvestris's current Linux chrome is client-side and GPUI-managed, so the PTY layer must not depend on compositor details. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs) and [GPUI platform source](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 1. POSIX PTY lifecycle

Recommended sequence:

1. compute initial terminal rows/columns;
2. call `openpty` with initial `winsize`;
3. set close-on-exec on the master;
4. preserve master in parent;
5. fork/launch child with a child pre-exec path;
6. child:
   - restore/default signal handlers;
   - `setsid()`;
   - `ioctl(slave, TIOCSCTTY, ...)`;
   - duplicate slave to stdin/stdout/stderr;
   - close original master/slave descriptors;
   - exec shell;
7. parent:
   - close slave;
   - start reader/writer/waiter paths on master/child PID.

Ghostty's `src/pty.zig` implements the core PTY half of this pattern and is the main source reference. See [Ghostty PTY](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig).

## 2. Rust process implementation

Two reasonable implementations:

**`openpty` + `fork`/`exec` wrappers** — explicit and easy to map to terminal semantics, but requires careful `unsafe`/post-fork restrictions.

**`std::process::Command` with Unix `pre_exec`** — keeps higher-level process construction in Rust but the pre-exec closure runs after fork and must obey async-signal-safe constraints.

Choose the route that produces the smallest auditable unsafe boundary in the actual codebase. Do not do allocation, logging, mutex locking or arbitrary Rust work in a post-fork/pre-exec child path.

The platform module should expose a safe spawned-process/PTY owner after the unsafe creation phase.

## 3. Shell selection

MVP:

1. use explicit configured/CLI command when provided;
2. otherwise use `$SHELL` if present and executable;
3. fallback `/bin/sh`.

Do not parse `/etc/passwd`, probe desktop-specific shells, or install shell integration.

Launch as a normal interactive shell. Login-shell behavior can be an explicit later choice; prefixing `argv[0]` with `-` affects shell startup semantics and should not be done accidentally.

Preserve the user's environment and locale. Add only terminal identity variables required by the app.

## 4. Environment

Start conservatively:

```text
TERM=xterm-256color
COLORTERM=truecolor
```

Do not claim `xterm-ghostty`. `libghostty-vt` may understand protocols that this GPUI renderer intentionally does not implement. The TERM value is a contract to terminal applications, not a brand string.

Preserve `LANG`, `LC_*`, `PATH`, `HOME`, `SHELL`, desktop/session variables and user environment. Do not force UTF-8 locale by rewriting environment; terminal bytes are still handled as a byte stream by Ghostty.

GPUI has its own `GraphicalEnvironment` handling for Wayland/X11/session variables when launching graphical children from an app, but the interactive shell should generally inherit the process environment. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 5. PTY mode

Ghostty's PTY code explicitly enables `IUTF8` because it is not guaranteed everywhere. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig). Mirror only PTY settings required for correct terminal behavior; do not impose canonical/echo mode. The child shell/application controls those through termios.

The master remains a raw byte transport from the app's perspective.

## 6. Reader and writer

The same shared architecture as Windows applies.

Reader thread:

```text
read(master_fd, reusable buffer)
 -> lock GhosttyTerminal
 -> ghostty_terminal_vt_write
 -> unlock
 -> generation/wakeup
```

Writer thread:

```text
ordered queue
 -> write(master_fd, bytes)
 -> handle partial writes / EINTR
```

Do not put the master FD into nonblocking mode merely because terminals often do; blocking worker threads are intentionally acceptable for a single session. If close/unblock behavior is problematic, use poll/eventfd/nonblocking as a targeted improvement.

The GPUI foreground thread never calls blocking `read`, `write`, `waitpid` or terminal process setup.

## 7. `EINTR`, partial writes and EOF

Handle POSIX semantics correctly:

- retry `read`/`write` on `EINTR`;
- loop on partial writes;
- EOF/`EIO` from PTY master around child exit may be platform-specific; classify expected termination separately from runtime failure;
- do not spin on zero-byte reads;
- close FDs exactly once.

Add a small OS-I/O wrapper so these cases are testable.

## 8. Resize

When terminal rows/columns change:

```text
ioctl(master_fd, TIOCSWINSZ, &winsize)
```

Ghostty's current PTY source uses `TIOCSWINSZ` on the master. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig). The kernel signals the foreground process group as appropriate.

At the same logical resize, update `GhosttyTerminal` with rows/cols and cell pixel size. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Coalesce duplicate terminal sizes. Do not send every fractional GPUI size change as a new `winsize`.

## 9. Job control

Correct session/control-terminal setup matters because shells and full-screen TUIs rely on process groups, foreground jobs and signals. Missing `setsid`/`TIOCSCTTY` may appear to work for simple `echo` but fail for Ctrl+C, Ctrl+Z, job control or interactive editors.

Ghostty's child pre-exec code resets many signals to defaults, calls `setsid`, then sets the controlling terminal with `TIOCSCTTY`. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig). Treat that as a correctness checklist.

Do not manually forward Ctrl+C as Unix signals from the GUI; terminal Ctrl+C should normally become terminal input byte `0x03`, and the line discipline/foreground process group handles it.

## 10. Child reaping

Use a waiter thread or background task around `waitpid`. Reap exactly once. Record exit status/signal.

On child exit:

- mark session exited;
- wake GPUI;
- stop accepting new writes;
- close input/master as shutdown policy dictates;
- ensure reader exits;
- join workers off foreground thread.

Never leave zombies after repeated terminal open/close tests.

## 11. Linux windowing is not a backend concern

No `gtk`, `libwayland`, `xlib`, or compositor calls belong in `platform/linux.rs`. Silvestris/GPUI already own the window and current Silvestris comments document Linux client-side decorations. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

GPUI's platform layer detects/handles graphical environment and Wayland/X11. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs). Terminal input arrives as GPUI events, not raw evdev/XKB/Wayland events.

This separation is important because the PTY code should run identically under Wayland, X11 and nested/remote environments as long as GPUI can create a window.

## 12. Keyboard and IME

Use the same common GPUI/Ghostty input path as Windows. The Linux backend never translates key symbols to escape sequences. `GhosttyKeyEncoder` does that after the common layer maps GPUI key events to Ghostty key events.

IME/text input remains GPUI's platform `InputHandler` path. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

Test at least one Wayland and/or X11 setup available to development, especially modifiers, Alt, function keys, dead keys and composition.

## 13. Clipboard

Clipboard is GPUI/app-owned, not PTY-owned. Linux backend has no X11 selection/Wayland clipboard code. GPUI/platform services should abstract clipboard transport. The terminal paste still runs through Ghostty paste semantics. See [Ghostty paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c).

If primary-selection paste is desired later, add it as a UI/platform capability, not to POSIX PTY code.

## 14. Mouse

Linux PTY backend does nothing special for mouse. GPUI gives pointer events, common code converts coordinates/modes, Ghostty mouse encoder creates bytes, writer sends them to master FD. See [Ghostty mouse example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-encode-mouse/src/main.c).

## 15. Scrollback

Ghostty terminal state owns scrollback. Do not rely on kernel PTY scrollback; there is none for our purposes. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

## 16. Signals and shutdown

Normal window close should lead to terminal hangup semantics rather than an immediate arbitrary kill. Closing the PTY master typically causes the terminal relationship to end; define whether the app gives the shell a brief graceful-exit window before sending stronger termination.

Do not install process-global signal handlers unless the app actually needs them. GPUI or other libraries may already have signal expectations.

A forced termination fallback must target the child/process group deliberately; do not risk killing unrelated processes.

## 17. Linux packaging

The first package should depend on as little platform-specific runtime as Silvestris/GPUI already require plus the chosen Ghostty VT library. Do not add GTK just because Ghostty's Linux application uses GTK; our GUI is GPUI.

If statically linking `libghostty-vt` is practical, it minimizes ABI/runtime lookup issues. If dynamically linking, package the exact library version and ensure loader paths are deterministic.

Document required Zig only for building Ghostty VT from source if end users/developers need it.

## 18. Tests

### Unit

- shell selection fallback;
- winsize conversion/clamping;
- resize coalescing;
- writer retry on partial/EINTR;
- lifecycle state transitions;
- child exit status mapping.

### Integration/headless

- start `/bin/sh` in PTY;
- issue `printf` marker and read parsed terminal output;
- Ctrl+C running command;
- `stty size` changes after resize;
- alternate screen;
- Unicode;
- rapid output;
- clean EOF/exit;
- repeated spawn/close no zombie/no FD leak.

### GUI/manual

- terminal on Wayland;
- terminal on X11 if available;
- `vim`;
- `htop`/`btop`;
- mouse reporting;
- copy/paste;
- long scrollback;
- window live resize;
- DPI/scale change if environment permits.

## Deliverables

- Thin POSIX PTY backend.
- Correct controlling-terminal/job-control setup.
- Blocking I/O entirely off GPUI foreground thread.
- Ordered writer path.
- `TIOCSWINSZ` resize.
- Child reaping and deterministic teardown.
- No GUI/compositor dependency in backend.
- Conservative terminal environment.

## Avoidables

- GTK.
- Wayland/X11 direct code.
- nonblocking/epoll complexity before need.
- shell-integration installer.
- utmp/wtmp in MVP.
- login-shell assumptions.
- process-global signal machinery.
- `TERM=xterm-ghostty`.
- terminal parsing in transport.

## Regression traps

- master/slave descriptor retained on wrong side;
- missing CLOEXEC;
- child lacks controlling TTY;
- post-fork unsafe Rust work;
- EINTR treated as fatal;
- partial writes truncated;
- child never reaped;
- close leaves reader blocked;
- environment/locale overwritten;
- resize fails to reach foreground job;
- Wayland/X11 behavior accidentally coupled to PTY;
- Ctrl+C forwarded as process signal instead of terminal input.

## Source anchors

- Ghostty: [`src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
- Silvestris: [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs)
- GPUI: [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs), [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs)
