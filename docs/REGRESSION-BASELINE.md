# Regression baseline

Recorded at anchor commit `c9222d425d04a9dd29961871a091678ad985f2b1`, before
the `pain-cleanup-6` repair pass. These commands are the unchanged regression
baseline for the whole pass. They live in `scripts/ci.sh` and `scripts/ci.ps1`
and must keep working after each task, including after the terminal code moves
into reusable crates.

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features shell --locked -- -D warnings
cargo test --workspace --all-targets --features shell --locked
cargo test --release --test m6 --features shell --locked -- --nocapture
cargo build --release --features shell --locked
```

Additional real-boundary gates exercised on the appropriate host (not part of
plain `cargo test`, and deliberately not mocked):

- Windows ConPTY / PowerShell / Vim alternate-screen + mouse-mode gates
  (`tests/pty.rs`)
- Windows APC / Kitty graphics gate with the bundled ConPTY (`tests/pty.rs`)
- WSL APC gate, when a usable distro is installed (`tests/pty.rs`)
- Linux interactive PTY smoke / Kitty client gates (`tests/pty.rs`, `tests/kitty.rs`)
- GPUI headless render tests (`crates/pain-terminal-gpui/tests/gpui_render.rs`, `--features gpui-tests`)
- Normal interactive Pain use: typing, resize, scrollback, selection,
  copy/paste, IME/dead keys where available, mouse TUI, title changes, font
  resizing and clean shutdown (`docs/SMOKE.md`)

Do not delete or weaken these tests when files move into crates. A passing
automated check is supporting evidence, not complete verification.
