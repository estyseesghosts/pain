#!/usr/bin/env bash
# Local gate matching .github/workflows/verify.yml. No GUI is opened and no
# input is synthesized; see docs/SMOKE.md for the manual desktop checks.
set -euo pipefail
cd "$(dirname "$0")/.."

commit_date="$(git show -s --format=%cs HEAD)"
short_hash="$(git rev-parse --short=7 HEAD)"
if ! [[ "$commit_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || ! [[ "$short_hash" =~ ^[0-9a-f]{7}$ ]]; then
  echo "cannot derive a release version from HEAD: $commit_date $short_hash" >&2
  exit 1
fi
expected_version="v${commit_date:2:2}-${commit_date:5:2}-$short_hash"
echo "expected release version: $expected_version"

echo '== format =='
cargo fmt --all -- --check
echo '== clippy =='
cargo clippy --workspace --all-targets --features shell --locked -- -D warnings
echo '== tests =='
cargo test --workspace --all-targets --features shell --locked
echo '== performance counters =='
cargo test --release --test m6 --features shell --locked -- --nocapture
echo '== release build =='
cargo build --release --features shell --locked

# An isolated CARGO_TARGET_DIR lets this gate run while release windows from
# earlier smoke tests still have the default binary open.
target_dir="${CARGO_TARGET_DIR:-target}"
version_line="$("$target_dir/release/pain" --version | head -n 1)"
if [[ "$version_line" != "pain $expected_version "* ]]; then
  echo "release version mismatch: expected '$expected_version' in '$version_line'" >&2
  exit 1
fi
echo "version: $version_line"

if [[ "${1:-}" == "--gui" ]]; then
  echo '== opt-in window check =='
  ./scripts/smoke.sh --gui
else
  echo 'Skipping GUI launch (pass --gui when you are not using the desktop).'
fi

echo 'CI OK.'
