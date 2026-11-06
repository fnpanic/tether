#!/usr/bin/env bash
# Server key rotation on a VM relay (docs/key-rotation.md). As root, one relay at a time:
#   rotate-key.sh switch <new id_ed25519>   the new key signs, the current one stays accepted
#   rotate-key.sh retire                    stop accepting the old key (deletes id_ed25519.old)
#   rotate-key.sh rollback                  undo "switch": the old key signs again
# hbbs and hbbr read KEYS_ACCEPT from /etc/tether/keys.env; relays installed before key rotation
# get a systemd drop-in for it. Restarting hbbr drains its sessions (up to 300 s).
# Tests: DATA, ETC, BIN, UNITS point elsewhere and NO_RESTART=1 skips chown/systemctl.
set -euo pipefail
DATA="${DATA:-/var/lib/tether}"
ETC="${ETC:-/etc/tether}"
BIN="${BIN:-/usr/local/bin}"
UNITS="${UNITS:-/etc/systemd/system}"
[ -n "${NO_RESTART:-}" ] || [ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }

die() { echo "rotate-key: $*" >&2; exit 1; }
# Public half of a hbbs secret key file (base64 of 64 bytes: seed, then public key).
pub_of() {
  [ "$(base64 -d <"$1" 2>/dev/null | wc -c)" = 64 ] || die "$1 is not a hbbs key file (base64 of 64 bytes)"
  base64 -d <"$1" | tail -c 32 | base64 -w0
}
own() { [ -n "${NO_RESTART:-}" ] || chown tether:tether "$@"; }
accept() { printf 'KEYS_ACCEPT=%s\n' "$1" >"$ETC/keys.env"; chmod 0644 "$ETC/keys.env"; }
restart() {
  command -v restorecon >/dev/null && restorecon -R "$DATA" "$ETC" "$UNITS" 2>/dev/null || true
  [ -n "${NO_RESTART:-}" ] && return
  systemctl daemon-reload
  systemctl restart tether-hbbs
  echo "restarting hbbr (drains open sessions, up to 300 s)…"
  systemctl restart tether-hbbr
  systemctl is-active tether-hbbs tether-hbbr
}

for b in hbbs hbbr; do
  grep -aq KEYS_ACCEPT "$BIN/$b" || die "$BIN/$b has no KEYS_ACCEPT support: install the current relay binaries first"
  grep -qs keys.env "$UNITS/tether-$b.service" "$UNITS/tether-$b.service.d/"*.conf 2>/dev/null || {
    mkdir -p "$UNITS/tether-$b.service.d"
    printf '[Service]\nEnvironmentFile=-%s/keys.env\n' "$ETC" >"$UNITS/tether-$b.service.d/keys.conf"
  }
done

case "${1:-}" in
  switch)
    new="${2:?usage: rotate-key.sh switch <new id_ed25519>}"
    newpub="$(pub_of "$new")"
    cur="$(pub_of "$DATA/id_ed25519")"
    [ "$newpub" != "$cur" ] || die "$new is already the signing key"
    # A second switch would drop the key devices may still present from KEYS_ACCEPT.
    [ ! -f "$DATA/id_ed25519.old" ] || [ "$(pub_of "$DATA/id_ed25519.old")" = "$cur" ] ||
      die "a switch is in progress (id_ed25519.old): run retire or rollback first"
    cp -p "$DATA/id_ed25519" "$DATA/id_ed25519.old"
    cp -p "$DATA/id_ed25519.pub" "$DATA/id_ed25519.old.pub"
    install -m 0600 "$new" "$DATA/id_ed25519"
    printf '%s' "$newpub" >"$DATA/id_ed25519.pub"
    own "$DATA/id_ed25519" "$DATA/id_ed25519.pub" "$DATA/id_ed25519.old" "$DATA/id_ed25519.old.pub"
    accept "$cur"
    restart
    echo "signing with $newpub, still accepting $cur"
    ;;
  retire)
    accept ""
    rm -f "$DATA/id_ed25519.old" "$DATA/id_ed25519.old.pub"
    restart
    echo "accepting only $(cat "$DATA/id_ed25519.pub")"
    ;;
  rollback)
    [ -f "$DATA/id_ed25519.old" ] || die "no id_ed25519.old to roll back to"
    newpub="$(pub_of "$DATA/id_ed25519")"
    install -m 0600 "$DATA/id_ed25519.old" "$DATA/id_ed25519"
    pub_of "$DATA/id_ed25519" >"$DATA/id_ed25519.pub"
    own "$DATA/id_ed25519" "$DATA/id_ed25519.pub"
    accept "$newpub"
    restart
    echo "signing with $(cat "$DATA/id_ed25519.pub") again, still accepting $newpub"
    ;;
  *) die "usage: rotate-key.sh switch <new id_ed25519> | retire | rollback" ;;
esac
