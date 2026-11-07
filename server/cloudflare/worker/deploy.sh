#!/usr/bin/env bash
# Manual fallback for what Tether Manage does on every publish: deploy the release files in
# <dir> (laid out <channel>/<app>/<platform>/<file>, e.g. stable/customer/windows-x86_64/TetherQS-0.5.24.exe.gz)
# as the static assets of the Worker "tether-dl".
#   server/cloudflare/worker/deploy.sh <dir> [env file]   (env file default C:\vms\cloudflare\cf.env)
# The env file holds CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID (an account token; wrangler
# uses them without `wrangler login`). Its values are never printed.
# A version replaces every file: include the previous release too (clients mid-download, halts).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
src="${1:?usage: deploy.sh <dir> [env file]}"
envf="${2:-/c/vms/cloudflare/cf.env}"
[ -f "$envf" ] || { echo "missing $envf" >&2; exit 1; }
set -a; . "$envf"; set +a
: "${CLOUDFLARE_API_TOKEN:?not set in $envf}" "${CLOUDFLARE_ACCOUNT_ID:?not set in $envf}"
unset R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY R2_ENDPOINT
# 25 MiB per file is the Workers Static Assets limit: refuse before uploading anything.
big="$(find "$src" -type f -size +26214400c)"
[ -z "$big" ] || { echo "larger than 25 MiB (ship it gzip, or relay-only):" >&2; echo "$big" >&2; exit 1; }
rm -rf "$here/public" && mkdir -p "$here/public"
cp -r "$src"/. "$here/public/"
# Immutable, versioned file names; the clients verify every byte anyway.
printf '/*\n  Cache-Control: public, max-age=86400\n  X-Content-Type-Options: nosniff\n' > "$here/public/_headers"
cd "$here" && npx --yes wrangler@4 deploy
