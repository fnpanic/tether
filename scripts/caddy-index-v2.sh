#!/bin/bash
# On a relay: let Caddy route releases-v2.json to tether-api like releases.json (where the index
# route exists; without it Caddy serves both as files). Validated before reload, else restored.
#   scp scripts/caddy-index-v2.sh opc@<relay>:/tmp/ && ssh opc@<relay> bash /tmp/caddy-index-v2.sh
set -e
C=/etc/tether/Caddyfile
if ! sudo grep -q '@index path_regexp .*releases\\\.json\$' "$C"; then
  echo "no @index route for releases.json here (or already updated):"; sudo grep -n '@index' "$C" || true; exit 0
fi
sudo cp -p "$C" "$C.bak-v2"
sudo sed -i 's#/releases\\\.json\$#/releases(-v2)?\\.json$#' "$C"
sudo grep -n '@index' "$C"
if sudo caddy validate --config "$C" --adapter caddyfile >/dev/null 2>&1; then
  sudo systemctl reload tether-https && echo "caddy reloaded"
else
  sudo cp -p "$C.bak-v2" "$C"; echo "caddy config invalid, restored"; exit 1
fi
