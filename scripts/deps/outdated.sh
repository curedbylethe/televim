#!/usr/bin/env bash
#
# List pending dependency updates in three semver buckets.
#
#   outdated.sh             run the report: cargo update --dry-run (patch and
#                           minor) plus cargo outdated (major, when installed)
#   outdated.sh --classify  classify dry-run output on stdin, print the body
#
# Classification lives in one place, so the offline test and the live report
# read the same rows. Both exit 0: this is a report, not a gate.
#
# Bash 3.2 compatible: macOS ships that version as /bin/bash and the script is
# run through /usr/bin/env bash by contributors as well as CI.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
manifest="$root/Cargo.toml"

is_held() {
  case "$1" in
    glass_pumpkin|vim-line) return 0 ;;
    *) return 1 ;;
  esac
}

held_reason() {
  case "$1" in
    glass_pumpkin) printf '%s' 'pinned at 2.0.0-rc0 for grammers-crypto (AGENTS rule 10)' ;;
    vim-line) printf '%s' 'held at 7.7; tui::line works around the 7.x API (AGENTS rule 8)' ;;
  esac
}

# Keep only the leading numeric components: 2.0.0-rc0 -> "2 0 0 ".
numeric_components() {
  local v="$1" result=""
  while [[ $v =~ ^[0-9]+ ]]; do
    result+="${BASH_REMATCH[0]} "
    v="${v#${BASH_REMATCH[0]}}"
    v="${v#.}"
  done
  printf '%s' "$result"
}

# The first leading component that differs names the class: 0 major, 1 minor,
# the rest patch. A 0.x minor bump is breaking, but cargo update cannot emit one
# (0.x minors fall outside the manifest requirement), so component comparison
# is enough for its output.
classify_versions() {
  local old="$1" new="$2" i n
  local -a a b
  read -r -a a <<< "$(numeric_components "$old")"
  read -r -a b <<< "$(numeric_components "$new")"
  n=${#a[@]}
  if [ "$n" -gt "${#b[@]}" ]; then n=${#b[@]}; fi
  for ((i = 0; i < n; i++)); do
    if [ "$((10#${a[i]}))" -ne "$((10#${b[i]}))" ]; then
      case "$i" in
        0) printf '%s' major ;;
        1) printf '%s' minor ;;
        *) printf '%s' patch ;;
      esac
      return 0
    fi
  done
  if [ "${#a[@]}" -ne "${#b[@]}" ]; then printf '%s' patch; fi
  return 0
}

# Keys of [workspace.dependencies], minus the internal path crates.
direct_names() {
  awk '
    /^\[workspace\.dependencies\]/ { inside = 1; next }
    /^\[/ { inside = 0 }
    inside && $0 ~ /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
      key = $0
      sub(/[[:space:]]*=.*/, "", key)
      gsub(/[[:space:]]/, "", key)
      if (key != "domain" && key != "proto" && key != "tui" && key != "telegram-framework")
        print key
    }
  ' "$manifest"
}

# Newline-delimited sets. Bash 3.2 has no associative arrays, and a leading and
# trailing newline make exact membership a substring match.
contains_name() {
  [[ "$1" == *$'\n'"$2"$'\n'* ]]
}

print_bucket() {
  printf '%s (%s)\n' "$1" "$2"
  [ -n "$3" ] && printf '%s' "$3"
  [ -n "$4" ] && printf '%s' "$4"
  return 0
}

classify() {
  local direct_list=$'\n' seen_list=$'\n' d name old new cls row
  local patch_direct="" patch_trans="" minor_direct="" minor_trans=""
  local major_direct="" major_trans="" direct_rows=""
  local held_gp="" held_vl=""
  local patch_n=0 minor_n=0 major_n=0 new_n=0 updated_n=0 direct_n=0
  local line re_updating re_adding key
  re_updating='^[[:space:]]*Updating[[:space:]]+([^[:space:]]+)[[:space:]]+v([^[:space:]]+)[[:space:]]*->[[:space:]]*v([^[:space:]]+)'
  re_adding='^[[:space:]]*Adding[[:space:]]+([^[:space:]]+)[[:space:]]+v[^[:space:]]+'

  while IFS= read -r d; do direct_list+="$d"$'\n'; done < <(direct_names)

  while IFS= read -r line; do
    if [[ $line =~ $re_updating ]]; then
      name="${BASH_REMATCH[1]}"
      old="${BASH_REMATCH[2]}"
      new="${BASH_REMATCH[3]}"
      if is_held "$name"; then
        if [ "$name" = glass_pumpkin ] && [ -z "$held_gp" ]; then
          held_gp="held  $name  $old -> $new  ($(held_reason "$name"))"$'\n'
        elif [ "$name" = vim-line ] && [ -z "$held_vl" ]; then
          held_vl="held  $name  $old -> $new  ($(held_reason "$name"))"$'\n'
        fi
        continue
      fi
      # A repeated name keeps its first row; the later one is the same crate
      # reported again by cargo, not a second update.
      if contains_name "$seen_list" "$name"; then continue; fi
      cls=$(classify_versions "$old" "$new")
      [ -n "$cls" ] || continue
      seen_list+="$name"$'\n'
      row="$name  $old -> $new"
      case "$cls" in
        patch)
          patch_n=$((patch_n + 1))
          if contains_name "$direct_list" "$name"; then
            patch_direct+="$row"$'\n'; direct_rows+="$row"$'\n'; direct_n=$((direct_n + 1))
          else
            patch_trans+="$name"$'\n'; updated_n=$((updated_n + 1))
          fi
          ;;
        minor)
          minor_n=$((minor_n + 1))
          if contains_name "$direct_list" "$name"; then
            minor_direct+="$row"$'\n'; direct_rows+="$row"$'\n'; direct_n=$((direct_n + 1))
          else
            minor_trans+="$name"$'\n'; updated_n=$((updated_n + 1))
          fi
          ;;
        major)
          major_n=$((major_n + 1))
          if contains_name "$direct_list" "$name"; then
            major_direct+="$row"$'\n'; direct_rows+="$row"$'\n'; direct_n=$((direct_n + 1))
          else
            major_trans+="$name"$'\n'; updated_n=$((updated_n + 1))
          fi
          ;;
      esac
    elif [[ $line =~ $re_adding ]]; then
      new_n=$((new_n + 1))
    fi
    # Removing, Downgrading and Locking lines carry no update to classify.
  done

  # Major rows come from the optional cargo outdated leg, not from dry-run.
  if [ "${OUTDATED_MAJOR_MEASURED:-1}" = "1" ] && [ -n "${OUTDATED_MAJOR:-}" ]; then
    while read -r name old new; do
      [ -n "${name:-}" ] || continue
      if contains_name "$seen_list" "$name" || is_held "$name"; then continue; fi
      cls=$(classify_versions "$old" "$new")
      [ "$cls" = major ] || continue
      seen_list+="$name"$'\n'
      major_n=$((major_n + 1))
      row="$name  $old -> $new"
      if contains_name "$direct_list" "$name"; then
        major_direct+="$row"$'\n'; direct_rows+="$row"$'\n'; direct_n=$((direct_n + 1))
      else
        major_trans+="$name"$'\n'; updated_n=$((updated_n + 1))
      fi
    done <<< "$OUTDATED_MAJOR"
  fi

  print_bucket patch "$patch_n" "$patch_direct" "$patch_trans"
  print_bucket minor "$minor_n" "$minor_direct" "$minor_trans"
  print_bucket major "$major_n" "$major_direct" "$major_trans"
  if [ "${OUTDATED_MAJOR_MEASURED:-1}" != "1" ]; then
    printf 'major: unmeasured here (install: cargo install cargo-outdated)\n'
  fi
  key=""
  if [ -n "$held_gp" ]; then key+="$held_gp"; fi
  if [ -n "$held_vl" ]; then key+="$held_vl"; fi
  held_n=0
  [ -n "$held_gp" ] && held_n=$((held_n + 1))
  [ -n "$held_vl" ] && held_n=$((held_n + 1))
  printf 'held (%s)\n' "$held_n"
  printf '%s' "$key"
  printf 'direct (%s)\n' "$direct_n"
  printf '%s' "$direct_rows"
  printf 'transitive: %s new, %s updated\n' "$new_n" "$updated_n"
}

case "${1:-report}" in
  --classify)
    classify
    ;;
  report)
    cd "$root"
    if dry_out=$(cargo update --dry-run 2>&1); then
      :
    else
      printf '%s\n' "$dry_out"
      printf 'report incomplete\n'
      exit 0
    fi

    # cargo outdated sees releases outside the manifest requirement. Absent, the
    # major bucket is reported as unmeasured rather than silently empty.
    measured=1
    major_rows=""
    if command -v cargo-outdated >/dev/null 2>&1; then
      if major_out=$(cargo outdated --root-deps-only --depth 1 2>&1); then
        major_rows=$(printf '%s\n' "$major_out" | awk '
          $1 ~ /^[A-Za-z0-9_]/ && $2 ~ /^[0-9]/ && $3 ~ /^[0-9]/ { print $1, $2, $3 }
        ')
      else
        measured=0
      fi
    else
      measured=0
    fi

    printf '%s\n' "$dry_out" \
      | OUTDATED_MAJOR="$major_rows" OUTDATED_MAJOR_MEASURED="$measured" "$here/outdated.sh" --classify \
      || true
    ;;
  *)
    printf 'usage: %s [--classify]\n' "$0" >&2
    exit 2
    ;;
esac

exit 0
