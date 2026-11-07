#!/usr/bin/env bash
# One /q/ download through Caddy (headers + SHA-256), for tether-api from <bin-dir> and the
# Caddyfile in <caddyfile> (default: this checkout's).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; BIN="$1"
W=/tmp/lt-qchk; "$HERE/stack.sh" stop "$W" 2>/dev/null || true; rm -rf "$W"
mkdir -p "$W/api/custom" "$W/api/download/stable/customer/windows-x86_64"
TOK=abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQ
d="$W/api/download/stable/customer/windows-x86_64"
head -c 3000000 /dev/urandom > "$d/TetherQS-9.9.9.exe"
echo '{"version":"9.9.9","platforms":{"windows-x86_64":{"signature":"x","url":"https://r/download/stable/customer/windows-x86_64/TetherQS-9.9.9.exe"}}}' > "$d/latest.json"
echo "{\"id\":\"acme\",\"name\":\"ACME\",\"token\":\"$TOK\",\"expires\":$(( $(date +%s) + 86400 )),\"channel\":\"stable\",\"created\":1,\"downloads\":0,\"last_download\":null,\"profile\":{\"x\":1}}" > "$W/api/custom/acme.json"
CADDYFILE="${2:-}" "$HERE/stack.sh" start "$BIN" "$W" >/dev/null
curl -sk -D "$W/h" -o "$W/body" "https://localhost:28443/q/$TOK"
grep -iE "^(HTTP|content-type|content-disposition|content-length|cache-control|x-accel)" "$W/h" | tr -d '\r'
echo "body $(stat -c %s "$W/body") bytes, starts with exe: $(cmp -n 3000000 "$W/body" "$d/TetherQS-9.9.9.exe" >/dev/null && echo yes || echo no), sha $(sha256sum < "$W/body" | cut -c1-16)"
for u in /download/../qcache/acme--TetherQS-9.9.9.exe /download/%2e%2e/qcache/acme--TetherQS-9.9.9.exe /qcache/acme--TetherQS-9.9.9.exe; do
  curl -sk --path-as-is -o /dev/null -w "$u: %{http_code} %{size_download} bytes\n" "https://localhost:28443$u"
done
"$HERE/stack.sh" stop "$W"
