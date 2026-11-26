# Packaging and Installation

This produces one executable and no sidecar data files. The `libghostty-vt`
library is built from the pinned vendored source and linked statically; fonts
and chrome come from the pinned Silvestris/GPUI crates.

## Prerequisites

- Rust 1.98.1 (see `rust-toolchain.toml`).
- Zig 0.17.x. Set `ZIG` to the compiler path when it is not on `PATH`.
- Windows: GNU host toolchain. This development host also needs a `windres`
  shim ahead of `PATH` because MSYS2 `windres` mangles GPUI's Windows resource
  path; see [docs/SOURCE-MAP.md](SOURCE-MAP.md). It does not change any upstream
  file.
- Linux: normal development headers for `libc`; the GUI needs WSLg (or a native
  Wayland/X11 session). Builds and tests must run on ext4, not `/mnt/c`.

## Windows release

```powershell
cd C:\Users\terezi\workdir\ghostty\pain
$env:ZIG = 'C:\path\to\zig.exe'
$env:PATH = 'C:\Users\terezi\AppData\Local\Temp\opencode\windres-shim;' + $env:PATH
cargo build --release --features shell
```

The binary is `target\release\pain.exe`. Close any running `pain.exe` before
rebuilding, otherwise Cargo cannot replace the locked file. Copy the executable
wherever it will be launched; there is no installer or registry step. Close the
window to exit; the child shell is reaped and the console-less build leaves no
dangling console.

The executable carries the app icon as its first Windows icon resource, so
Explorer, the taskbar and the window use it. `build.rs` compiles
`resources\windows\pain.rc` with `windres` and links the object into the binary;
the shim on `PATH` is needed for the icon for the same reason as GPUI's manifest.
`resources\windows\pain.ico` is checked in with 16/24/32/48/64/128/256 px
entries, generated from the `pain.png` master and its `0.25x`/`0.5x` exports in
`~\Documents\app-icons\pain`. MSVC builds skip the resource; the GNU toolchain
above is the Windows release target.

## Linux release (native or WSLg)

```sh
cd /home/terezi/workdir/ghostty/pain
export ZIG=/home/terezi/zig-linux/zig
cargo build --release --features shell
install -Dm755 target/release/pain ~/.local/bin/pain
```

Under WSLg, run it with a GUI display available:

```sh
export DISPLAY=:0 WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000
~/.local/bin/pain
```

There is no `.desktop` file in this release; launch it from a shell or a
terminal launcher. The app uses the same GPUI/Silvestris window code on Windows
and Linux.

## Versioning

Release tags and `pain --version` use `vYY-MM-xxxxxxx`: the last two digits of
the commit year, the two-digit commit month, and Git's seven-character short
hash, e.g. `v26-11-0566384`. This matches Chaus's tag shape. `build.rs` computes
the string from HEAD and embeds it as `PAIN_RELEASE_VERSION`; a source archive
with no `.git` gets the obviously synthetic `v00-00-unknown`.
`crates/ghostty-vt-sys/build.rs` passes Ghostty's pinned app version to Zig
explicitly, so Ghostty's own `vX.Y.Z` tag validation never inspects pain's tag.

Zig builds the pinned static `libghostty-vt` on both Windows toolchains. The
GNU host uses MinGW `windres` for the icon; GitHub's Windows runner stays on
the native MSVC host because GPUI's `windows-manifest` feature emits an MSVC
import library there, and `build.rs` finds `rc.exe` through `embed-resource`.
Both paths embed the same icon resource.

## Local and GitHub CI

Run the complete local gate from a checkout:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/ci.ps1
```

```sh
./scripts/ci.sh
```

Both scripts check formatting, clippy with `-D warnings`, the full test suite,
the M6 stress counters, a release build, and that `--version` names HEAD. They
do not open a window; pass `-Gui` or `--gui` to opt into the existing smoke
launch check.

`.github/workflows/verify.yml` runs the same gate on Ubuntu and Windows for
branches and pull requests. `.github/workflows/release.yml` reruns it for a
`vYY-MM-xxxxxxx` tag, builds `pain-linux-x64` and `pain-windows-x64.exe` with
`.sha256` files, checks that each binary prints the tagged version, and
publishes the assets with `gh release create --verify-tag`. A manual
`workflow_dispatch` with an existing tag rebuilds a failed release.

## Command-line surface

```text
pain [--profile <id>] [--shell <program> [args...]] [-- <program> [args...]]
     [--font-size <pixels>] [--scrollback-lines <n>]
     [--scrollback-bytes <n>] [--version] [--help]
```

- The default profile is PowerShell 7 when installed, then Windows PowerShell,
  then `cmd.exe`; Linux uses `$SHELL`/`/bin/sh`. A profile saved in Preferences
  replaces the automatic choice. `--profile <id>` selects one launch, and
  `--shell`/`--` overrides both.
- Windows installs a taskbar jump list at startup with one `New <profile>
  window` task per detected shell, each running `pain --profile <id>`.
- `--font-size` sets the starting logical pixel size; `Ctrl+=`, `Ctrl+-` and
  `Ctrl+0` still work at runtime.
- Scrollback caps default to 10,000 lines and 16 MiB and are applied to Ghostty
  at session creation. Zero disables a cap; Ghostty still prunes by page.
- `--version` prints the app version and the linked `libghostty-vt` version.
  On Windows the GUI-subsystem binary has no console, so the same line is
  written to the log file.

## Logging

The Windows GUI build has no stderr. Startup, shell spawn, resize, copy/paste
and PTY write errors are appended to `%LOCALAPPDATA%\pain\pain.log`. On Linux
the destination is `$XDG_DATA_HOME/pain/pain.log` (or `~/.local/share/pain/pain.log`),
and `PAIN_LOG` overrides it on both platforms. Nothing is logged per cell or
per frame.

## Version and source attribution

- App version: `PAIN_RELEASE_VERSION` from `build.rs`, formatted as
  `vYY-MM-xxxxxxx` by `src/version.rs`.
- Linked engine: `libghostty-vt` release `vt-1.0.2`, commit
  `573c4a21e02311ae87844af26b6a7a179f82858e`, vendored in `vendor/ghostty-vt`.
  `PIN.txt` and `crates/ghostty-vt-sys/build.rs` refuse a mismatched snapshot.
- Silvestris `devel/sil-cocoa-fixture` and the exact Zed/GPUI revision it pins
  are recorded in [docs/SOURCE-MAP.md](SOURCE-MAP.md).
- `pain --version` reports the runtime `ghostty_version()` string, so a binary
  can be checked without reading build logs.

## Unsupported features

See [docs/SUPPORTED.md](SUPPORTED.md). The short version: one window, one
session, no tabs/splits/workspaces/multi-pane settings/plugins/images/macOS
build. Shell profiles are a single default-shell choice, not saved window
layouts.

## Release checks

Run [the smoke script checklist](SMOKE.md) on both platforms before shipping.
It covers launch, shell, resize, copy/paste, scrollback, title and clean exit.
