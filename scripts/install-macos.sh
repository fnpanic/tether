#!/bin/sh
# Install Tether or TetherQS on macOS without the Gatekeeper "Open Anyway" step, at no cost.
# Files fetched with curl carry no com.apple.quarantine flag, and Gatekeeper only assesses
# quarantined files (docs/code-signing.md). Trust comes from HTTPS to our own relay plus a strict
# codesign check of the bundle, the same as a browser download (which Gatekeeper rejects anyway,
# since the apps are not notarized).
#
#   curl -fsSL https://<relay>/download/install-macos.sh | sh -s -- https://<relay> [technician|customer]
#   curl -fsSL https://<relay>/download/install-macos.sh | sh -s -- 'https://<relay>/q/<token>/mac'
set -eu

src=${1:?usage: install-macos.sh https://<relay> [technician|customer] | <file URL>}
role=${2:-technician}
case $role in technician|customer) ;; *) echo "role must be technician or customer" >&2; exit 2 ;; esac
case $src in https://*) ;; *) echo "only https:// URLs" >&2; exit 2 ;; esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

case $src in
  */q/*|*.zip|*.tar.gz) url=$src ;; # a custom app link (<relay>/q/<token>/mac) or a release file
  *)
    # latest.json: {"version":..,"platforms":{"darwin-universal":{"signature":..,"url":".."}}}
    url=$(curl -fsSL "${src%/}/download/stable/$role/darwin-universal/latest.json" | tr -d '\n\r\t ' |
      sed -n 's/.*"darwin-universal":{[^}]*"url":"\([^"]*\)".*/\1/p')
    [ -n "$url" ] || { echo "no macOS release found at $src for $role" >&2; exit 1; }
    ;;
esac

echo "Downloading $url"
curl -fL --proto '=https' --tlsv1.2 -o "$tmp/pkg" "$url"
mkdir "$tmp/x"
if [ "$(head -c 2 "$tmp/pkg")" = "PK" ]; then ditto -x -k "$tmp/pkg" "$tmp/x"; else tar -xzf "$tmp/pkg" -C "$tmp/x"; fi

app=$(find "$tmp/x" -maxdepth 1 -name '*.app' -type d | head -n 1)
[ -n "$app" ] || { echo "no .app in the download" >&2; exit 1; }
id=$(defaults read "$app/Contents/Info.plist" CFBundleIdentifier)
case $id in app.tether.desktop|net.tether.quick) ;; *) echo "unexpected bundle id: $id" >&2; exit 1 ;; esac
codesign --verify --deep --strict "$app"

dest=/Applications
[ -w "$dest" ] || { dest="$HOME/Applications"; mkdir -p "$dest"; }
name=$(basename "$app")
pkill -x "$(defaults read "$app/Contents/Info.plist" CFBundleExecutable)" 2>/dev/null || true
rm -rf "${dest:?}/$name"
ditto "$app" "$dest/$name"
xattr -dr com.apple.quarantine "$dest/$name" 2>/dev/null || true # in case an older copy carried it
echo "Installed $dest/$name ($id)"
open "$dest/$name"
