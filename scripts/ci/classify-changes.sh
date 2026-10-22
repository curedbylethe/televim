#!/usr/bin/env bash
#
# Decide whether a CI run needs the Rust gate.
#
#   classify-changes.sh paths   changed paths on stdin, one per line
#   classify-changes.sh git     the event named by EVENT/BEFORE/BASE
#
# Both print `true` or `false`. Documentation is a path ending `.md` or sitting
# under `docs/`; every other path is code. Anything unknown — no event, no base
# commit, an empty diff — is code too: skipping the gate hides a broken tree,
# while an extra run only costs minutes.
set -euo pipefail

any_code() {
  local seen=0 path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    seen=1
    case "$path" in
      *.md|docs/*) ;;
      *) return 0 ;;
    esac
  done
  [ "$seen" -eq 0 ]
}

base=""
case "${1:-git}" in
  paths)
    if any_code; then echo true; else echo false; fi
    ;;
  git)
    case "${EVENT:-}" in
      pull_request) base="${BASE:-}" ;;
      push) base="${BEFORE:-}" ;;
      *) echo true; exit 0 ;;
    esac

    # An all-zero `before` is a new branch; a missing object is a force-push
    # past this clone. Neither can be diffed.
    if [ -z "$base" ] || [ -z "${base//0/}" ] || ! git cat-file -e "$base^{commit}" 2>/dev/null; then
      echo "no base commit to diff against" >&2
      echo true
      exit 0
    fi

    changed=$(git diff --name-only "$base" HEAD) || { echo true; exit 0; }
    if printf '%s\n' "$changed" | any_code; then echo true; else echo false; fi
    ;;
  *)
    echo "usage: $0 {paths|git}" >&2
    exit 2
    ;;
esac
