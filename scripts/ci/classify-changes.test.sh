#!/usr/bin/env bash
#
# Table test for classify-changes.sh: the path rule decides whether the Rust
# gate runs, so it is checked here rather than trusted. Prints only failures.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
classify="$here/classify-changes.sh"
status=0

check() {
  local want="$1" label="$2"
  shift 2
  local got
  got=$(printf '%s\n' "$@" | "$classify" paths)
  if [ "$got" != "$want" ]; then
    printf 'FAIL %s: want %s, got %s\n' "$label" "$want" "$got" >&2
    status=1
  fi
}

check false 'markdown at the root' README.md
check false 'markdown under docs' docs/testing.md
check false 'an image under docs' docs/diagram.png
check false 'a dotfile under docs' docs/.gitkeep
check true  'a Rust source' crates/tui/src/lib.rs
check true  'a build script whose directory starts with docs' docs-src/build.rs
check true  'docs plus code' docs/testing.md crates/app/src/main.rs
check true  'a workflow' .github/workflows/ci.yml
check true  'an empty diff'

exit "$status"
