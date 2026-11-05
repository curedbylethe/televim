#!/bin/sh
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
server_pid=
cleanup() {
  [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup 0 HUP INT TERM

server=$tmp/server
version_dir=$server/v9.9.9
mkdir -p "$version_dir" "$tmp/home"
cat > "$tmp/televim" <<'EOF'
#!/bin/sh
printf 'televim 9.9.9\n'
EOF
chmod 755 "$tmp/televim"
printf 'fixture\n' > "$tmp/README.md"
printf 'fixture\n' > "$tmp/LICENSE-MIT"
printf 'fixture\n' > "$tmp/LICENSE-APACHE"

asset=televim-9.9.9-x86_64-unknown-linux-gnu.tar.gz
tar -czf "$version_dir/$asset" -C "$tmp" televim README.md LICENSE-MIT LICENSE-APACHE
if command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$version_dir/$asset" | awk '{print $1}')
else
  digest=$(shasum -a 256 "$version_dir/$asset" | awk '{print $1}')
fi
printf '%s  %s\n' "$digest" "$asset" > "$version_dir/SHA256SUMS"

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$port" --directory "$server" --bind 127.0.0.1 > "$tmp/http.log" 2>&1 &
server_pid=$!
base=http://127.0.0.1:$port
ready=false
attempt=0
while [ "$attempt" -lt 40 ]; do
  if curl -fsS "$base/v9.9.9/SHA256SUMS" -o /dev/null 2>/dev/null; then ready=true; break; fi
  attempt=$((attempt + 1))
  sleep 0.1
done
[ "$ready" = true ] || { cat "$tmp/http.log" >&2; exit 1; }

install_dir=$tmp/home/.local/bin
env HOME="$tmp/home" SHELL=/bin/bash PATH="$PATH" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.9 TELEVIM_INSTALL_DIR="$install_dir" sh "$here/install.sh"
[ "$("$install_dir/televim" --version)" = 'televim 9.9.9' ]
[ "$(stat -c '%a' "$install_dir/televim" 2>/dev/null || stat -f '%Lp' "$install_dir/televim")" = 755 ]
[ "$(grep -Fc '# >>> televim installer >>>' "$tmp/home/.profile")" -eq 1 ]
env HOME="$tmp/home" SHELL=/bin/bash PATH="$PATH" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.9 TELEVIM_INSTALL_DIR="$install_dir" sh "$here/install.sh" >/dev/null
[ "$(grep -Fc '# >>> televim installer >>>' "$tmp/home/.profile")" -eq 1 ]
path_home=$tmp/path-home
mkdir -p "$path_home"
env HOME="$path_home" SHELL=/bin/bash PATH="$install_dir:$PATH" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.9 TELEVIM_INSTALL_DIR="$install_dir" sh "$here/install.sh" >"$tmp/path.out"
[ ! -e "$path_home/.profile" ]
grep -q '^Run: televim --version$' "$tmp/path.out"
if grep -q '^Add this line' "$tmp/path.out"; then
  echo 'installer printed PATH setup instructions for a directory already on PATH' >&2
  exit 1
fi
cp "$tmp/home/.profile" "$tmp/profile.before"
env HOME="$tmp/home" SHELL=/bin/bash PATH="$PATH" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.9 TELEVIM_INSTALL_DIR="$install_dir" sh "$here/install.sh" --no-modify-path >/dev/null
cmp "$tmp/profile.before" "$tmp/home/.profile"

mkdir "$server/v9.9.8"
bad_asset=televim-9.9.8-x86_64-unknown-linux-gnu.tar.gz
cp "$version_dir/$asset" "$server/v9.9.8/$bad_asset"
printf '%064d  %s\n' 0 "$bad_asset" > "$server/v9.9.8/SHA256SUMS"
if env HOME="$tmp/home" SHELL=/bin/bash PATH="$PATH" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.8 TELEVIM_INSTALL_DIR="$tmp/bad-bin" sh "$here/install.sh" >"$tmp/bad.out" 2>&1; then
  echo 'checksum mismatch unexpectedly succeeded' >&2
  exit 1
fi
grep -q 'checksum mismatch' "$tmp/bad.out"
[ ! -e "$tmp/bad-bin/televim" ]

if env HOME="$tmp/home" TELEVIM_OS=Plan9 TELEVIM_ARCH=amd64 sh "$here/install.sh" >"$tmp/unsupported.out" 2>&1; then
  echo 'unsupported platform unexpectedly succeeded' >&2
  exit 1
fi
grep -q 'aarch64-apple-darwin' "$tmp/unsupported.out"

if env HOME="$tmp/home" TELEVIM_OS=Linux TELEVIM_ARCH=x86_64 TELEVIM_BASE_URL="$base" TELEVIM_VERSION=9.9.7 sh "$here/install.sh" >"$tmp/missing.out" 2>&1; then
  echo 'missing asset unexpectedly succeeded' >&2
  exit 1
fi
grep -q 'televim-9.9.7-x86_64-unknown-linux-gnu.tar.gz' "$tmp/missing.out"

sh -n "$here/install.sh"
if command -v shellcheck >/dev/null 2>&1; then shellcheck "$here/install.sh"; fi
if grep -Eq '(^|[[:space:]])sudo([[:space:]]|$)' "$here/install.sh"; then
  echo 'installer must not use sudo' >&2
  exit 1
fi
