#!/usr/bin/env bash
#
# Table test for outdated.sh --classify: the bucket split decides what a reader
# acts on, so it is checked here rather than trusted. Prints PASS/FAIL per row.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
classify="$here/outdated.sh"

fixture='    Updating serde_json v1.0.140 -> v1.0.141
    Updating emojis v0.9.0 -> v0.9.1
    Updating tokio v1.45.0 -> v1.46.0
    Updating glass_pumpkin v2.0.0-rc0 -> v2.0.0
    Updating prerelease_crate v1.0.0-rc1 -> v1.0.0
      Adding ar_archive_writer v0.5.5
     Locking 5 packages to latest compatible versions
    Updating emojis v0.9.0 -> v0.9.9
'

# The classifier must not depend on the repository's live workspace manifest.
fixture_manifest="$(mktemp)"
trap 'rm -f "$fixture_manifest"' EXIT
printf '[workspace.dependencies]\nserde_json = "1"\nemojis = "0.9"\ntokio = "1"\nprerelease_crate = "1"\n' > "$fixture_manifest"

# Name Project Compat Latest: a major-only pending release has Compat `---`.
major_fixture='serde_json 1.0.140 --- 2.0.0'

out=$(printf '%s\n' "$fixture" | OUTDATED_MANIFEST="$fixture_manifest" OUTDATED_MAJOR="$major_fixture" "$classify" --classify)
status=0

check() {
  local want="$1" label="$2" got
  got=$(printf '%s\n' "$out" | grep -F -- "$want" || true)
  if [ -n "$got" ]; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s: missing %s\n' "$label" "$want" >&2
    status=1
  fi
}

absent() {
  local unwanted="$1" label="$2" got
  got=$(printf '%s\n' "$out" | grep -F -- "$unwanted" || true)
  if [ -z "$got" ]; then
    printf 'PASS %s\n' "$label"
  else
    printf 'FAIL %s: unexpected %s\n' "$label" "$unwanted" >&2
    status=1
  fi
}

check 'patch (3)'  'patch bucket count'
check 'minor (1)'  'minor bucket count'
check 'major (1)'  'major bucket count'
check 'held (1)'   'held bucket count'
check 'direct (5)' 'direct bucket count'
check 'transitive: 1 new, 0 updated' 'transitive summary'
check 'serde_json  1.0.140 -> 1.0.141' 'stable patch row'
check 'emojis  0.9.0 -> 0.9.1' '0.x patch row'
check 'tokio  1.45.0 -> 1.46.0' 'minor row'
check 'prerelease_crate  1.0.0-rc1 -> 1.0.0' 'pre-release to release patch row'
check 'serde_json  1.0.140 -> 2.0.0' 'major row with Compat ---'
check 'held  glass_pumpkin  2.0.0-rc0 -> 2.0.0' 'held row with reason'
absent 'v0.9.9'          'first-seen duplicate wins'
absent 'ar_archive_writer' 'Adding makes no row'
absent 'Locking 5'       'Locking line ignored'

exit "$status"
