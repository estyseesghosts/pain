#!/usr/bin/env bash
# Regression smoke check for the Linux/WSLg release. Does not synthesize input.
set -euo pipefail
cd "$(dirname "$0")/.."

echo '== headless and integration tests =='
cargo test --workspace --all-targets --features shell

echo '== performance stress counters =='
cargo test --release --test m6 --features shell -- --nocapture

echo '== release build =='
cargo build --release --features shell
exe=target/release/pain

if [[ "${1:-}" != "--gui" ]]; then
  echo 'Skipping GUI launch (pass --gui to opt in when you are not using the desktop).'
elif [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
  echo 'No display exported; skipping GUI launch. Set DISPLAY/WAYLAND_DISPLAY for WSLg.'
else
  echo '== launch and close check (foregrounds a window; opt-in only) =='
  "$exe" &
  pid=$!
  sleep 4
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "pain exited immediately; check ${XDG_DATA_HOME:-$HOME/.local/share}/pain/pain.log" >&2
    exit 1
  fi
  kill "$pid"
  wait "$pid" 2>/dev/null || true
fi

echo 'SMOKE OK: tests, counters and release build passed.'
echo 'Manual checks: docs/SMOKE.md'
