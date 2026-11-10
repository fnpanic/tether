#!/usr/bin/env bash
# Roll the relay bundle dist/linux-relay out to running relays, one at a time (docs/deployment.md,
# "Update: review fixes"): hbbs, tether-api with its unit, the Caddyfile and the ddns unit.
# hbbr is left alone unless HBBR=1 (its restart ends the relayed sessions on that node).
#   bash scripts/deploy-relay-update.sh                 # both production relays, the backup first
#   RELAYS="user@host=ssh-key-name" bash scripts/deploy-relay-update.sh
# Stops at the first relay that fails a check; the old files stay as *.bak-review on the node.
set -euo pipefail
cd "$(dirname "$0")/../dist"
# user@host=key file in ~/.ssh (docs/relay-ha.md: Oracle relay 2 is the backup, Hetzner the primary).
RELAYS=${RELAYS:-"opc@92.5.231.71=tether_oracle tadmin@tether-desk.duckdns.org=tether_hetzner"}
B=linux-relay
grep " \*\./$B/" SHA256SUMS | sha256sum -c --quiet -   # the bundle is the one that was tested

for e in $RELAYS; do
  r=${e%%=*} SSH_KEY=$HOME/.ssh/${e#*=}
  echo "== $r"
  ssh -i "$SSH_KEY" "$r" 'install -d -m 0700 ~/tether-update'
  scp -q -i "$SSH_KEY" $B/bin/hbbs $B/bin/hbbr $B/bin/tether-api $B/systemd/tether-api.service \
    $B/systemd/tether-ddns.service $B/caddy/Caddyfile "$r:tether-update/"
  ssh -i "$SSH_KEY" "$r" HBBR="${HBBR:-0}" 'bash -s' <<'EOF'
set -euo pipefail
cd ~/tether-update
RELAY_HOST=$(sudo sed -n 's/^RELAY_HOST=//p' /etc/tether/server.env)   # not sourced: RELAY_SERVERS holds |
# The new hbbs/hbbr exit when they cannot read the key; tether-api needs its own host name.
sudo -u tether test -r /var/lib/tether/id_ed25519 || { echo "tether cannot read /var/lib/tether/id_ed25519: fix owner/mode first"; exit 1; }
[ -n "${RELAY_HOST:-}" ] || { echo "RELAY_HOST missing in /etc/tether/server.env"; exit 1; }
systemctl is-active -q tether-https || { echo "tether-https (Caddy) is not active: nginx nodes are updated by hand"; exit 1; }
sudo RELAY_HOST="$RELAY_HOST" /usr/local/bin/caddy validate --config Caddyfile --adapter caddyfile >/dev/null

put() { # file target mode
  [ -e "$2.bak-review" ] || sudo cp -p "$2" "$2.bak-review"
  sudo install -m "$3" "$1" "$2" && sudo restorecon "$2"
}
put tether-api /usr/local/bin/tether-api 0755
put tether-api.service /etc/systemd/system/tether-api.service 0644
put Caddyfile /etc/tether/Caddyfile 0644
put hbbs /usr/local/bin/hbbs 0755
sudo systemctl daemon-reload
sudo systemctl restart tether-api tether-hbbs
sudo systemctl restart tether-https            # WebSockets through Caddy reconnect within seconds
if [ "$HBBR" = 1 ]; then put hbbr /usr/local/bin/hbbr 0755; sudo systemctl restart tether-hbbr; fi
if systemctl cat tether-ddns.service >/dev/null 2>&1; then
  put tether-ddns.service /etc/systemd/system/tether-ddns.service 0644
  sudo systemctl daemon-reload
  # DynamicUser under SELinux is untested: put the old unit back if the update does not run.
  sudo systemctl start tether-ddns.service && journalctl -u tether-ddns -n 4 --no-pager | grep -q "duckdns: OK" || {
    echo "ddns unit failed: old unit restored"
    sudo install -m 0644 /etc/systemd/system/tether-ddns.service.bak-review /etc/systemd/system/tether-ddns.service
    sudo systemctl daemon-reload
  }
fi

sleep 5
for u in tether-hbbs tether-hbbr tether-api tether-https; do systemctl is-active -q $u || { echo "$u is NOT active"; exit 1; }; done
if sudo journalctl -u tether-api -n 20 --no-pager | grep -q "RELAY_HOST is not set"; then echo "tether-api did not get RELAY_HOST"; exit 1; fi
curl -fsS localhost:21122/readyz >/dev/null
code=$(curl -s -o /dev/null -w '%{http_code}' "https://$RELAY_HOST/api/policy")
case $code in 200|404) ;; *) echo "https://$RELAY_HOST/api/policy answered $code"; exit 1;; esac
rm -rf ~/tether-update
echo "ok: $RELAY_HOST updated"
EOF
done
# Rollback on a relay: for f in /usr/local/bin/hbbs /usr/local/bin/hbbr /usr/local/bin/tether-api \
#   /etc/systemd/system/tether-api.service /etc/systemd/system/tether-ddns.service /etc/tether/Caddyfile; do
#   [ -e $f.bak-review ] && sudo install -m "$(stat -c %a $f)" $f.bak-review $f; done
#   sudo systemctl daemon-reload && sudo systemctl restart tether-api tether-hbbs tether-hbbr tether-https
