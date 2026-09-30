#!/usr/bin/env bash
#
# Keep this repository and the OpenDesign project in step.
#
#   design-sync.sh check   diff both ways; non-zero if any file differs
#   design-sync.sh pull    OpenDesign -> this repository   (after a design run)
#   design-sync.sh push    this repository -> OpenDesign   (after editing here)
#
# This repository is the versioned record. OpenDesign keeps its own opaque store of
# UUID-named files, which is an undo history rather than a reviewable one: no
# branches, no diff on a screen, no pull request. So the direction of truth is
# always *into* git — `pull` imports a design run's work, and `push` only ever
# carries an edit made here out to the design surface so the two do not drift.
#
# The path map is explicit below rather than discovered, because a file that
# vanished from one side should be an error and not a silent skip.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
project="${OPEN_DESIGN_PROJECT:-$HOME/Library/Application Support/Open Design/namespaces/release-stable/data/projects/televim-app}"

# repo-relative path : project-relative path
PAIRS=(
  "DESIGN.md:design-system/DESIGN.md"
  "design/index.html:index.html"
  "design/televim-engine.js:televim-engine.js"
  "design/televim-ui.js:televim-ui.js"
  "design/televim.css:televim.css"
  "design/design-system/tokens.css:design-system/tokens.css"
  "design/design-system/components.html:design-system/components.html"
  "design/design-system/build-specimen.js:design-system/build-specimen.js"
)

die() { printf 'design-sync: %s\n' "$1" >&2; exit 1; }

if [ "${1:-}" = "" ] || [ $# -ne 1 ]; then
  die "usage: design-sync.sh {check|pull|push}"
fi
mode="$1"

[ -d "$project" ] || die "no OpenDesign project at
  $project
Set OPEN_DESIGN_PROJECT to a project on this machine, or see design/README.md."

# Every mode needs both sides: `check` diffs them, `pull` reads the project, and
# `push` writes it. A file missing on either side is an error rather than a skip,
# because a skip is how two copies of a screen drift without anybody noticing.
for pair in "${PAIRS[@]}"; do
  repo_rel="${pair%%:*}"
  od_rel="${pair##*:}"
  [ -f "$here/$repo_rel" ] || die "missing here: $repo_rel"
  [ -f "$project/$od_rel" ] || die "missing in the project: $od_rel"
done

# move <direction> <label>: copy every file that differs, and say how many actually
# moved rather than how many are in the map. A count that always reads as the size
# of the map is a count nobody can trust.
#
# The two sides have *different* paths for the same file, so the pair is always
# read as `repo_rel` under `$here` and `od_rel` under `$project` — never crossed.
# Crossing them is a bug that copies nothing, reports that it copied everything, and
# leaves the drift in place.
move() {
  local direction="$1" label="$2" moved=0 pair repo_rel od_rel
  for pair in "${PAIRS[@]}"; do
    repo_rel="${pair%%:*}"
    od_rel="${pair##*:}"
    diff -q "$here/$repo_rel" "$project/$od_rel" >/dev/null 2>&1 && continue

    if [ "$direction" = "pull" ]; then
      cp "$project/$od_rel" "$here/$repo_rel"
    else
      cp "$here/$repo_rel" "$project/$od_rel"
    fi
    printf '  %s  %s\n' "$label" "$repo_rel"
    moved=$((moved + 1))
  done

  if [ "$moved" -eq 0 ]; then
    printf 'design-sync: nothing to %s; %d file(s) already in step.\n' "$direction" "${#PAIRS[@]}"
  else
    printf 'design-sync: %s %d of %d file(s).\n' "$direction" "$moved" "${#PAIRS[@]}"
  fi
}

case "$mode" in
  check)
    drifted=0
    for pair in "${PAIRS[@]}"; do
      repo_rel="${pair%%:*}"
      od_rel="${pair##*:}"
      if ! diff -q "$here/$repo_rel" "$project/$od_rel" >/dev/null 2>&1; then
        drifted=$((drifted + 1))
        printf '  differs: %-42s <-> %s\n' "$repo_rel" "$od_rel"
      fi
    done
    if [ "$drifted" -eq 0 ]; then
      printf 'design-sync: the repository and %s are in step (%d files)\n' \
        "$(basename "$project")" "${#PAIRS[@]}"
      exit 0
    fi
    printf 'design-sync: %d of %d files differ.\n' "$drifted" "${#PAIRS[@]}" >&2
    printf '  A design run edits the OpenDesign project, so `make design-pull` is\n' >&2
    printf '  usually what you want. `make design-push` carries an edit made here\n' >&2
    printf '  out to the design surface instead.\n' >&2
    exit 1
    ;;

  pull) move pull pulled ;;
  push) move push pushed ;;

  *) die "unknown mode: $mode" ;;
esac
