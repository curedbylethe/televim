#!/bin/sh
set -eu

dist=crates/web-demo/dist
[ -d "$dist" ] || { echo "dist missing"; exit 1; }

if grep -R -a -n -E '(TELEGRAM_API_(ID|HASH)|TELEVIM_API_(ID|HASH)|TELEVIM_SESSION_PASSPHRASE|api_id|api_hash|session_key)[[:space:]]*[:=][[:space:]]*[A-Za-z0-9_./+-]{6,}' "$dist"; then
  echo "credential-shaped string in dist"
  exit 1
fi

if find "$dist" \( -name '*.map' -o -name '*.dSYM' \) -print | grep -q .; then
  echo "debug artifact in dist"
  exit 1
fi

if find "$dist" -type f \( -name '*.html' -o -name '*.js' -o -name '*.css' -o -name '*.txt' \) \
  -exec grep -n -i -E '\b(phone|password|passcode)\b' {} + | grep -v -i -E 'disabled|no session|not available|demo'; then
  echo "unexpected credential UI copy"
  exit 1
fi

if env | grep -q -E '^(TELEGRAM_API_ID|TELEGRAM_API_HASH|TELEVIM_API_ID|TELEVIM_API_HASH|TELEVIM_SESSION_PASSPHRASE)='; then
  echo "credential environment variable present during build"
  exit 1
fi
