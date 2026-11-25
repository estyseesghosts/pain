# macOS / OS X Backend — Deferred Design

**Status: deferred, not built, not tested, not a release target.**

This document exists only to keep the cross-platform seam honest while development is Windows/Linux-only. No macOS-specific implementation should be merged or described as supported until there is a real Mac test machine or reliable macOS CI capable of exercising the application. The intended visual inspiration may be legacy Mac OS X Terminal.app, but the active binaries are Windows and Linux.

The future transport is conceptually straightforward because Ghostty's current PTY implementation uses the POSIX PTY path on non-Windows systems, including macOS, and `libghostty-vt` is cross-platform. See [Ghostty PTY](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig) and [Ghostty README](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/README.md). The UI side is less safe to predict: current Silvestris explicitly says macOS is not yet a target in its custom-chrome comments and its README says Windows is the current development platform while macOS/Linux validation remains open. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs) and [README](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/README.md). GPUI does contain macOS platform support, but the terminal must be tested against the exact pinned revision before any claims. See [GPUI platform source](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 1. Why this file exists

The Windows/Linux implementation should not bake assumptions such as:

```rust
enum PlatformPty {
    ConPty(...),
    Linux(...),
}
```

Instead, use a platform module boundary where a future `macos.rs` can satisfy the same transport contract as Linux without changing terminal/session/render code.

Likewise, common key/text logic must not equate “Alt” behavior with Windows/Linux semantics. Ghostty's key encoder has a macOS-specific `option-as-alt` setting that is not derivable from terminal state and is reset by `ghostty_key_encoder_set_from_terminal`; a future macOS implementation must set that option according to app policy after terminal synchronization. See [Ghostty key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h).

## 2. Expected PTY transport

Likely future path:

- `openpty`;
- CLOEXEC master;
- `IUTF8`;
- child session via `setsid`;
- controlling terminal via `TIOCSCTTY`;
- slave duplicated to stdio;
- master reader/writer workers;
- resize with `TIOCSWINSZ`;
- child wait/reap.

Ghostty's current POSIX PTY source is the checklist. See [Ghostty `src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig).

Do not share Linux-specific assumptions such as filesystem paths or signal quirks by copy/paste. Factor only genuinely POSIX-common code after a Mac build can validate it.

## 3. UI/window behavior

Current Silvestris `chrome.rs` comments say macOS would use system-drawn traffic lights at `traffic_light_position`, whereas Windows/Linux use other custom/client-side behavior. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs). A future terminal should continue to consume Silvestris `WindowFrame`/window APIs rather than adding an AppKit terminal window itself.

Do not write AppKit code now based on old assumptions about GPUI's macOS implementation.

## 4. Text input and Option key

macOS text input has important differences:

- Option may generate composed characters or act as Alt depending on user preference;
- dead keys and IME composition must flow through GPUI `InputHandler`;
- Command shortcuts belong to application routing, not terminal Ctrl semantics;
- candidate window positioning still needs terminal cursor geometry.

GPUI's platform `InputHandler` remains the intended text boundary. See [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs). Ghostty's key encoder exposes macOS option-as-alt policy. See [Ghostty key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h).

Do not guess the correct default before testing real macOS behavior and comparing to legacy/current Terminal.app/Ghostty.

## 5. Rendering

The renderer should remain identical: Ghostty dirty render state → one GPUI custom element. No Metal-specific terminal renderer is required. Ghostty leaves drawing to the embedder in `libghostty-vt`; see [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). GPUI's `Element` lifecycle is cross-platform at the app level; see [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

This is a major payoff of not embedding Ghostty's native renderer: adding macOS later should primarily be PTY/input/packaging validation, not a renderer rewrite.

## 6. Font behavior

Do not assume a specific Apple font exists or that GPUI text shaping behaves identically to Windows/Linux. Silvestris currently bundles its own font families and supports font overrides. See [Silvestris consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

A future Mac build should verify:

- monospace metrics;
- Retina scale;
- font fallback;
- emoji;
- combining marks;
- bold/italic fallback;
- candidate IME positioning;
- baseline parity with Windows/Linux.

## 7. Clipboard and menus

macOS may eventually use native menu projection through Silvestris. That is not relevant to the current Windows/Linux MVP. The terminal command model should nevertheless keep Copy/Paste actions separate from platform key spelling so Command+C/V can be assigned later without changing terminal semantics.

Program-initiated OSC clipboard access remains a security-policy question across every platform.

## 8. Packaging

No macOS package, signing, notarization, universal binary or app bundle work belongs in the current project. Do not add build dependencies or CI jobs that are never executed.

When macOS becomes active, first prove:

1. Silvestris current branch builds/runs on target macOS;
2. the pinned GPUI revision behaves correctly;
3. chosen `libghostty-vt` tag builds for the target;
4. POSIX PTY child/job control works;
5. input/Option/IME behavior is correct.

Only then design packaging.

## Future deliverables

- A real macOS build on tested hardware/CI.
- POSIX PTY backend reuse only where validated.
- Option/Alt policy.
- Command vs Ctrl routing.
- native text/IME smoke tests.
- Retina/DPI/font tests.
- close/process lifecycle tests.
- packaging only after runtime correctness.

## Current avoidables

- Any `#[cfg(target_os = "macos")]` implementation added solely from this design note.
- AppKit/Swift bridge.
- Ghostty Metal renderer.
- inferred legacy Terminal.app behavior presented as fact.
- macOS CI configuration that nobody can run.
- universal/signing/notarization work.
- Apple-specific fonts relied on by Windows/Linux common code.

## Regression traps for the future

- Reusing Linux POSIX code that performs non-portable child-pre-exec behavior.
- Option key always treated as Alt and breaking character composition.
- Command shortcuts leaking into terminal encoding.
- different backing scale invalidating cached rows.
- native traffic lights/titlebar hitboxes occluded by terminal element.
- claiming support because it compiles without testing IME/job control/resize.

## Source anchors

- Ghostty: [`src/pty.zig`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/src/pty.zig), [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [key encoder](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/key/encoder.h)
- Silvestris: [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`README.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/README.md), [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md)
- GPUI: [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs), [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`text_system.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs)
