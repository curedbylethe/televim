#!/bin/sh
set -eu
umask 022

INSTALL_DIR=${TELEVIM_INSTALL_DIR:-"${HOME:?HOME must be set}/.local/bin"}
BASE_URL=${TELEVIM_BASE_URL:-https://github.com/curedbylethe/televim/releases/download}
MODIFY_PATH=true
TEMP_DIR=

usage() {
  cat <<EOF
Install televim from a verified release archive.
Usage: install.sh [--no-modify-path] [--help]
Supported targets: macOS arm64/x86_64, Linux x86_64/arm64 (GNU libc).
Default install directory: $INSTALL_DIR
Profile line: export PATH="$INSTALL_DIR:\$PATH"
Set TELEVIM_VERSION to select a release or TELEVIM_INSTALL_DIR to change the
install directory. Restart the shell after installation.
EOF
}

fail() {
  printf 'televim installer: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  [ -z "$TEMP_DIR" ] || rm -rf "$TEMP_DIR"
}
trap cleanup 0 HUP INT TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --help|-h) usage; exit 0 ;;
    --no-modify-path) MODIFY_PATH=false ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

case "$BASE_URL" in
  https://*) ;;
  http://127.0.0.1:*|http://localhost:*) ;;
  *) fail 'release downloads require HTTPS' ;;
esac

if command -v curl >/dev/null 2>&1; then
  DOWNLOADER=curl
elif command -v wget >/dev/null 2>&1; then
  DOWNLOADER=wget
else
  fail 'install curl or wget, then run this installer again'
fi
command -v tar >/dev/null 2>&1 || fail 'install tar, then run this installer again'
if command -v sha256sum >/dev/null 2>&1; then
  CHECKSUM=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  CHECKSUM=shasum
else
  fail 'install sha256sum or shasum, then run this installer again'
fi

OS=${TELEVIM_OS:-$(uname -s)}
ARCH=${TELEVIM_ARCH:-$(uname -m)}
case "$OS:$ARCH" in
  Darwin:arm64|Darwin:aarch64) TARGET=aarch64-apple-darwin ;;
  Darwin:x86_64|Darwin:amd64) TARGET=x86_64-apple-darwin ;;
  Linux:x86_64|Linux:amd64) TARGET=x86_64-unknown-linux-gnu ;;
  Linux:aarch64|Linux:arm64) TARGET=aarch64-unknown-linux-gnu ;;
  *) fail "unsupported platform $OS/$ARCH; supported targets: aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu" ;;
esac

RELEASES_URL=https://github.com/curedbylethe/televim/releases
VERSION=${TELEVIM_VERSION:-}
if [ -z "$VERSION" ]; then
  VERSION=
  if [ "$DOWNLOADER" = curl ]; then
    redirect=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$RELEASES_URL/latest" 2>/dev/null || true)
    case "$redirect" in */tag/v*) VERSION=${redirect##*/tag/v} ;; esac
  fi
  if [ -z "$VERSION" ]; then
    api_url=https://api.github.com/repos/curedbylethe/televim/releases/latest
    if [ "$DOWNLOADER" = curl ]; then
      response=$(curl -fsSL "$api_url") || fail "could not resolve latest release: $api_url"
    else
      response=$(wget -q -O - "$api_url") || fail "could not resolve latest release: $api_url"
    fi
    VERSION=$(printf '%s\n' "$response" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p' | head -n 1)
  fi
fi
VERSION=${VERSION#v}
printf '%s\n' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || fail "invalid release version: $VERSION"

case "$INSTALL_DIR" in
  /*) ;;
  *) INSTALL_DIR=$(pwd -P)/$INSTALL_DIR ;;
esac
ASSET=televim-$VERSION-$TARGET.tar.gz
ASSET_URL=$BASE_URL/v$VERSION/$ASSET
CHECKSUMS_URL=$BASE_URL/v$VERSION/SHA256SUMS
TEMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/televim-install.XXXXXX") || fail 'could not create a temporary directory'

download() {
  url=$1
  destination=$2
  if [ "$DOWNLOADER" = curl ]; then
    curl -fsSL "$url" -o "$destination" || fail "download failed: $url"
  else
    wget -q -O "$destination" "$url" || fail "download failed: $url"
  fi
}

download "$ASSET_URL" "$TEMP_DIR/$ASSET"
download "$CHECKSUMS_URL" "$TEMP_DIR/SHA256SUMS"
EXPECTED=$(awk -v asset="$ASSET" '$2 == asset || $2 == "*" asset { print $1 }' "$TEMP_DIR/SHA256SUMS")
case "$EXPECTED" in
  ''|*[!0-9A-Fa-f]*) fail "SHA256SUMS has no valid checksum for $ASSET" ;;
esac
[ "$(printf '%s' "$EXPECTED" | wc -c | tr -d ' ')" -eq 64 ] || fail "SHA256SUMS has no valid checksum for $ASSET"
if [ "$CHECKSUM" = sha256sum ]; then
  ACTUAL=$(sha256sum "$TEMP_DIR/$ASSET" | awk '{print $1}')
else
  ACTUAL=$(shasum -a 256 "$TEMP_DIR/$ASSET" | awk '{print $1}')
fi
[ "$(printf '%s' "$EXPECTED" | tr 'A-F' 'a-f')" = "$(printf '%s' "$ACTUAL" | tr 'A-F' 'a-f')" ] || fail "checksum mismatch for $ASSET"

tar -tzf "$TEMP_DIR/$ASSET" > "$TEMP_DIR/entries" || fail "invalid archive: $ASSET"
while IFS= read -r entry; do
  case "$entry" in
    /*|..|../*|*/..|*/../*) fail "unsafe archive path in $ASSET: $entry" ;;
  esac
done < "$TEMP_DIR/entries"
expected_entries=$(printf '%s\n' LICENSE-APACHE LICENSE-MIT README.md televim)
actual_entries=$(LC_ALL=C sort "$TEMP_DIR/entries")
[ "$actual_entries" = "$expected_entries" ] || fail "unexpected archive contents: $ASSET"
mkdir "$TEMP_DIR/extract"
tar -xzf "$TEMP_DIR/$ASSET" -C "$TEMP_DIR/extract" televim
[ -f "$TEMP_DIR/extract/televim" ] && [ ! -L "$TEMP_DIR/extract/televim" ] && [ -x "$TEMP_DIR/extract/televim" ] || fail "archive has no executable televim: $ASSET"
VERSION_OUTPUT=$("$TEMP_DIR/extract/televim" --version 2>&1) || fail 'downloaded binary could not run; check its architecture and runtime libraries'
case "$VERSION_OUTPUT" in
  "televim $VERSION") ;;
  *) fail "downloaded binary reported an unexpected version: $VERSION_OUTPUT" ;;
esac

mkdir -p "$INSTALL_DIR" || fail "cannot create install directory: $INSTALL_DIR"
[ -w "$INSTALL_DIR" ] || fail "install directory is not writable: $INSTALL_DIR"
if command -v install >/dev/null 2>&1; then
  install -m 755 "$TEMP_DIR/extract/televim" "$INSTALL_DIR/televim" || fail "could not install to $INSTALL_DIR"
else
  if cp "$TEMP_DIR/extract/televim" "$INSTALL_DIR/televim"; then
    chmod 755 "$INSTALL_DIR/televim" || fail "could not install to $INSTALL_DIR"
  else
    fail "could not install to $INSTALL_DIR"
  fi
fi

path_has_install_dir() {
  saved_ifs=$IFS
  IFS=:
  set -f
  for path_entry in $PATH; do
    if [ "$path_entry" = "$INSTALL_DIR" ]; then
      set +f
      IFS=$saved_ifs
      return 0
    fi
  done
  set +f
  IFS=$saved_ifs
  return 1
}

escaped_dir=$(printf '%s' "$INSTALL_DIR" | sed 's/[\\"$]/\\&/g')
export_line="export PATH=\"$escaped_dir:\$PATH\""
shell_name=${SHELL:-}
shell_name=${shell_name##*/}

write_profile() {
  path_profile=$1
  mkdir -p "$(dirname "$path_profile")" 2>/dev/null || true
  if [ -w "$(dirname "$path_profile")" ] || [ -w "$path_profile" ]; then
    if [ "${SHELL##*/}" = fish ]; then
      path_block=$(printf '# >>> televim installer >>>\nfish_add_path "%s"\n# <<< televim installer <<<' "$INSTALL_DIR")
    else
      # The generated shell block needs the literal PATH variable.
      # shellcheck disable=SC2016
      path_block=$(printf '# >>> televim installer >>>\ncase ":$PATH:" in\n  *":%s:"*) ;;\n  *) %s ;;\nesac\n# <<< televim installer <<<' "$escaped_dir" "$export_line")
    fi
    if ! grep -Fq '# >>> televim installer >>>' "$path_profile" 2>/dev/null; then
      printf '%s\n' "$path_block" >> "$path_profile" 2>/dev/null || printf 'Warning: could not update %s. Add this line to your shell profile:\n%s\n' "$path_profile" "$export_line" >&2
    fi
  else
    printf 'Warning: could not update %s. Add this line to your shell profile:\n%s\n' "$path_profile" "$export_line" >&2
  fi
}

if [ "$MODIFY_PATH" = false ]; then
  printf 'Add this line to your shell profile if needed:\n%s\n' "$export_line"
elif ! path_has_install_dir; then
  case "$shell_name" in
    zsh) write_profile "$HOME/.zshrc" ;;
    bash) write_profile "$HOME/.bashrc"; write_profile "$HOME/.profile" ;;
    fish) write_profile "$HOME/.config/fish/config.fish" ;;
    *) write_profile "$HOME/.profile" ;;
  esac
fi

INSTALLED_VERSION=$("$INSTALL_DIR/televim" --version 2>&1) || fail "binary installed at $INSTALL_DIR/televim but could not run; check architecture and libdbus-1"
case "$INSTALLED_VERSION" in
  "televim $VERSION") ;;
  *) fail "installed binary reported an unexpected version: $INSTALLED_VERSION" ;;
esac
printf 'Installed televim %s at %s/televim\n' "$VERSION" "$INSTALL_DIR"
if [ "$MODIFY_PATH" = false ]; then
  printf 'Run: %s/televim --version\n' "$INSTALL_DIR"
elif ! path_has_install_dir; then
  printf 'Restart your shell, then run: televim --version\n'
else
  printf 'Run: televim --version\n'
fi
