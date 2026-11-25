# Silvestris Integration

The terminal application is a **consumer of Silvestris**, not a terminal feature added to the Silvestris toolkit. The current Silvestris consumer documentation defines the ownership split directly: Silvestris owns generic controls/window chrome/themes/command routing/focus/modality/text-editing machinery; the application owns domain models, sessions and app-specific focus/input policy; GPUI owns platform paint/input. It explicitly lists domain canvases such as emulator screens and code editors as application-owned views. A terminal surface is another such domain canvas. See [Silvestris `docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

Ghostty reinforces this separation: its VT library owns terminal state but leaves drawing and PTY transport to the embedder. See [Ghostty `terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h). GPUI supplies the custom element used by that domain view. See [GPUI `element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs).

## 1. Dependency contract

Pin the terminal app to a specific Silvestris commit and use **the exact GPUI revision that Silvestris pins**. At the researched Silvestris revision `0d5cc609171992d003994c8d8fd44b014b4d0b94`, the workspace pins GPUI/Zed to `6b720973027999bae402df23d469026f4cced988`. Silvestris's consumer guide explicitly warns that two GPUI revisions in one build produce incompatible types. See [Silvestris `Cargo.toml`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/Cargo.toml) and [consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

Do not add a second direct Zed/GPUI revision for terminal-specific convenience. If a needed GPUI fix exists only upstream, move the Silvestris pin deliberately and validate Silvestris consumers rather than overriding it in the terminal app.

## 2. Application startup

The terminal app should follow the standard public consumer path:

```rust
gpui_platform::application().run(|cx| {
    silvestris::init(theme_preference, cx)?;
    let options = silvestris::chrome::window_options(
        "Terminal",
        initial_bounds,
        minimum_size,
        cx,
    );

    silvestris::chrome::open_window(options, cx, |window, cx| {
        cx.new(|cx| TerminalRoot::new(window, cx))
    })?;
})
```

`silvestris::init` currently registers bundled fonts, installs theme state, binds text editing keys and installs modal handling. See [Silvestris `lib.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/lib.rs). `chrome::open_window` applies native frame configuration and observes platform appearance. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

The terminal should not create raw GPUI windows directly unless a missing Silvestris API is proven to be a toolkit gap.

## 3. Window composition

Use `WindowFrame` as the outer app chrome:

```text
WindowFrame("Terminal")
└── terminal content area
    └── TerminalView
        └── TerminalElement
```

No toolbar is required for MVP. No sidebar, inspector or shell layout is required. `WindowFrame::child` accepts arbitrary content, so the terminal can remain a domain view without forcing it through Silvestris controls. See [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

If the implementation wants the title to show the shell/terminal OSC title, keep a stable fallback such as `Terminal` and update the `WindowFrame`/native title on the foreground thread.

## 4. Do we use `Shell`?

Probably not initially. Silvestris `Shell` is designed around sidebar/workspace/inspector composition and uses GPUI `container_query` to obtain measured allocated size. See [`shell.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/shell.rs). A one-canvas terminal does not need those regions.

The relevant lesson from `Shell` is architectural: measured GPUI allocation should drive domain layout. The terminal element similarly derives rows/columns from its actual allocated bounds. Do not introduce a separate window-size model and try to make GPUI obey it.

If the app later adds a tiny status bar or toolbar, `WindowFrame` can compose those without adopting the full sidebar/inspector shell.

## 5. Appearance and terminal palette

Use Silvestris theme tokens for **application UI**:

- window/chrome surfaces;
- scrollbar chrome if added;
- dialogs/alerts;
- selection focus indication around the terminal if needed;
- default font role and UI text.

Do not remap ANSI terminal colors through the Silvestris Aqua palette. Ghostty's render state provides terminal foreground/background/palette and per-cell resolved colors. See [Ghostty `render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h). A shell choosing red/green/blue should render those terminal colors even if the application is Aqua Dark.

The default terminal background/foreground can be initialized from a small terminal theme chosen to complement Aqua, but once configured those values belong to terminal settings, not arbitrary `Theme::colors.window`.

## 6. Font choice

Silvestris bundles a monospace font role; its current consumer docs note Aqua uses Source Code Pro in place of Monaco and Geist uses Geist Mono. See [Silvestris consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md) and [font registration](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/fonts.rs). The MVP should default to the active theme's mono role rather than hard-code a platform font name.

GPUI should measure the resolved font and shape text; no terminal geometry constant should assume fixed pixel metrics. See [GPUI text system](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/text_system.rs).

Font overrides are a later configuration surface. The first app can expose a single compile/default font size and perhaps CLI override without a preferences panel.

## 7. Focus and responder chain

The terminal needs near-exclusive keyboard ownership when focused. Silvestris's responder chain contains `ResponderEntry::capture_keys`, whose source comment describes exactly the domain-canvas use case: a canvas can swallow every command except an allowlist so shell-owned shortcuts still propagate. See [Silvestris `responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs).

Recommended policy:

- terminal focus receives ordinary keydown/up and text input;
- terminal consumes Ctrl/Alt/function/navigation combinations by default;
- application may allow explicit window/app commands outward;
- copy/paste should be terminal-aware commands, not generic text-field behavior;
- text-editing shortcuts registered globally by `silvestris::init` must not treat the terminal as a Silvestris `TextField`.

This should be tested with Ctrl+C, Ctrl+V, Ctrl+Shift+C/V if chosen, Alt sequences, Escape and function keys on both platforms.

GPUI `FocusHandle` is the underlying keyboard-focus primitive. See [GPUI `app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs).

## 8. Clipboard

User-initiated copy/paste should use GPUI/platform clipboard APIs through the application. Silvestris may provide command/menu integration, but terminal selection and paste semantics remain app-owned because they depend on Ghostty terminal state.

Paste sequence:

```text
Silvestris command / keyboard gesture
    -> read GPUI clipboard text
    -> Ghostty terminal_paste
    -> ordered PTY writer queue
```

Do not route paste through Silvestris text editing core. Ghostty's paste API handles terminal modes such as bracketed paste. See [Ghostty paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c).

## 9. Menus and commands

MVP can have almost no visible menu UI on Windows/Linux. Still, define app commands cleanly if Silvestris requires command routing:

- Copy
- Paste
- Clear Scrollback (later if easy)
- Close Window / Quit

Do not expose dozens of terminal preferences as commands. The command system is useful for routing and enabled-state validation, not a reason to build a command palette.

If native/platform menu support differs on Linux/Windows, the terminal app should remain usable without a menu bar. Keyboard and context-menu access to copy/paste is enough for MVP.

## 10. Context menu

A small right-click menu is reasonable after core terminal input works:

- Copy (enabled only with selection)
- Paste (enabled only if clipboard has text)
- Select All (optional)
- Clear (later)

Use Silvestris popup/menu infrastructure rather than building custom floating UI. Do not let context-menu implementation precede correct right-click/mouse reporting policy; a TUI may legitimately own right-click when mouse tracking is active, so define modifier/selection behavior first.

## 11. Scrollbar

A Snow Leopard-style terminal likely benefits visually from the current Silvestris Aqua scrollbar. However, Silvestris's `Scrollbar` is built around GPUI `ScrollHandle`, while Ghostty owns terminal history/viewport and exposes its own scrollbar state (`total`, `offset`, `len`) plus viewport movement. See [Silvestris `controls/scrollbar.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/controls/scrollbar.rs) and [Ghostty terminal scroll APIs](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h).

Therefore, do not attach a fake GPUI scroll container to terminal rows. Options in order:

1. **MVP:** no visible scrollbar; wheel/keys scroll Ghostty viewport.
2. Add a tiny application adapter/painter reusing Silvestris Aqua metrics/art but driven by Ghostty scrollbar values.
3. If a generic external-scroll-model API would clearly benefit multiple Silvestris consumers, add it upstream to Silvestris—do not hack `ScrollHandle` state inside the terminal.

The terminal's scrollback remains correct even if the decorative scrollbar is absent.

## 12. Window chrome and native Windows behavior

Silvestris's current `chrome.rs` uses GPUI's transparent titlebar and `window_control_area` on Windows so minimize/maximize/close, Snap Layouts, Aero Snap and the system menu stay native. Linux uses client-side decorations and resize edges. See [Silvestris `chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs).

The terminal must not create its own HWND child or native titlebar just to host ConPTY. ConPTY is transport, not a visual console. GPUI/Silvestris remain the only window.

Silvestris centralizes unsafe platform window adaptation in `native.rs`. Keep terminal-specific Windows unsafe code in the terminal's `platform/windows.rs`; do not mix ConPTY details into Silvestris native frame code. See [Silvestris `native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs).

## 13. DPI and scale

Silvestris/GPUI already own logical/device pixel conversion for UI. Terminal geometry must recalculate when:

- window/content size changes;
- display scale/DPI changes;
- terminal font family/size changes;
- line-height policy changes.

Use GPUI's measured text/font data. Then pass integer cell pixel dimensions to Ghostty resize for terminal size reports. Do not query Win32 DPI independently in generic terminal code.

Wintty's raw Win32 example handles `WM_DPICHANGED` itself because it owns the HWND directly. Our app should not duplicate that path; GPUI owns the window. See [Wintty Win32 host](https://github.com/pawaca/wintty/blob/047cd33cd1a24602cc4887ac260e2f922943cac0/example/c-win32-terminal/src/main.c) for contrast.

## 14. Caret blink and reduced motion

Silvestris `native::caret_blink` already exposes system caret timing on Windows and a designed fallback elsewhere. See [Silvestris `native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs). Use it as the default if it makes terminal cursor behavior feel native, while Ghostty render state remains authoritative for whether the cursor should blink and what visual style it has.

Do not run a high-frequency animation timer when cursor is hidden, terminal is unfocused, or the window is not being presented if GPUI exposes visibility. GPUI's platform layer has window visibility semantics; see [GPUI `platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs).

## 15. Error states

A terminal launch failure should use simple Silvestris UI:

```text
Unable to start shell
[technical error]
[Close]
```

Do not build a status center. If the child exits normally, either close the window or leave a short “process exited” state according to one simple product rule. The first implementation may close automatically.

## 16. Preferences

No preferences window in MVP. Essentials may live as constants or tiny persisted settings:

- default shell command;
- font size;
- scrollback cap;
- light/dark appearance preference if the app needs to expose it.

Silvestris owns storage mechanics but the application owns preference meaning/location according to the consumer contract. See [Silvestris consumer guide](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md).

## 17. Testing Silvestris integration

Test:

- terminal receives focus on launch;
- clicking terminal restores focus after context menu;
- app-level allowlisted command still works;
- terminal Ctrl sequences are not intercepted by generic command routing;
- theme change invalidates UI chrome and appropriate terminal defaults without recoloring ANSI cells;
- DPI/font change recomputes cell geometry;
- window resize does not flood PTY resizes;
- close path does not hang while reader thread is active;
- Windows titlebar retains Snap/system behavior;
- Linux resize edges/chrome still work around the custom terminal element.

Silvestris's own docs note rendered GPUI tests do not prove native input/window semantics; preserve separate real-platform smoke tests. See [Silvestris README](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/README.md).

## Deliverables

- Standard Silvestris startup/window path.
- Terminal kept as an application domain view.
- One `WindowFrame` with no unnecessary shell regions.
- Theme/font integration without corrupting terminal colors.
- Focus/responder capture appropriate for a terminal.
- Clipboard/context-menu integration.
- Optional scrollbar only as a Ghostty viewport adapter.
- Clean separation between Silvestris platform chrome and PTY platform code.

## Avoidables

- Adding terminal emulator code to Silvestris.
- Patching/vendoring Silvestris in the app.
- Using a different GPUI revision.
- Reusing Silvestris `TextField` as terminal input.
- Treating terminal rows as a generic scroll container.
- Theme-remapping ANSI colors.
- Building toolbar/sidebar/preferences UI.
- Raw native windows inside the terminal canvas.

## Regression traps

- Silvestris shortcut layer consuming Ctrl sequences.
- Theme switch invalidating text font but not terminal row cache.
- Font override changes cell width without PTY resize.
- Window title effect called from background thread.
- Context menu stealing focus permanently.
- Scrollbar developing an independent viewport state.
- Windows native titlebar hit regions broken by terminal overlay/hitbox.
- Linux client-decoration resize events intercepted by full-window terminal hitbox.
- Generic text-editing keybindings firing inside terminal.

## Source anchors

- Ghostty: [`terminal.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/terminal.h), [`render.h`](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/include/ghostty/vt/render.h), [paste example](https://github.com/ghostty-org/ghostty/blob/64a6fedfd26d1fa687733f9f411db5eb8e205ebe/example/c-vt-paste/src/main.c)
- Silvestris: [`docs/consumers.md`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/docs/consumers.md), [`chrome.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/chrome.rs), [`responder.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/responder.rs), [`native.rs`](https://github.com/estyseesghosts/silvestris/blob/0d5cc609171992d003994c8d8fd44b014b4d0b94/crates/silvestris/src/native.rs)
- GPUI: [`element.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/element.rs), [`app.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/app.rs), [`platform.rs`](https://github.com/zed-industries/zed/blob/6b720973027999bae402df23d469026f4cced988/crates/gpui/src/platform.rs)
