#!/usr/bin/env bash
# Self-check of rotate-key.sh without root or systemd (Linux/WSL): bash server/rotate-key-test.sh
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
export DATA="$T/data" ETC="$T/etc" BIN="$T/bin" UNITS="$T/units" NO_RESTART=1
mkdir -p "$DATA" "$ETC" "$BIN" "$UNITS"
key() { head -c 64 /dev/urandom | base64 -w0 >"$1"; base64 -d <"$1" | tail -c 32 | base64 -w0; }
A="$(key "$DATA/id_ed25519")"; printf '%s' "$A" >"$DATA/id_ed25519.pub"
B="$(key "$T/new")"
printf 'old hbbs\n' >"$BIN/hbbs"; printf 'old hbbr\n' >"$BIN/hbbr"
printf '[Service]\nEnvironmentFile=-/etc/tether/server.env\n' >"$UNITS/tether-hbbs.service"
fail() { echo "FAIL: $*" >&2; exit 1; }

! bash "$HERE/rotate-key.sh" switch "$T/new" 2>/dev/null || fail "switched with binaries that lack KEYS_ACCEPT"
printf 'x KEYS_ACCEPT y\n' >"$BIN/hbbs"; printf 'x KEYS_ACCEPT y\n' >"$BIN/hbbr"
printf 'short' | base64 >"$T/bad"
! bash "$HERE/rotate-key.sh" switch "$T/bad" 2>/dev/null || fail "accepted a malformed key file"

bash "$HERE/rotate-key.sh" switch "$T/new" >/dev/null
[ "$(cat "$DATA/id_ed25519.pub")" = "$B" ] || fail "new public key not installed"
cmp -s "$DATA/id_ed25519" "$T/new" || fail "new secret key not installed"
[ "$(cat "$ETC/keys.env")" = "KEYS_ACCEPT=$A" ] || fail "old key not accepted"
grep -q "keys.env" "$UNITS/tether-hbbs.service.d/keys.conf" || fail "no drop-in for hbbs"
grep -q "keys.env" "$UNITS/tether-hbbr.service.d/keys.conf" || fail "no drop-in for hbbr"
[ "$(stat -c %a "$DATA/id_ed25519")" = 600 ] || fail "secret key not 0600"
! bash "$HERE/rotate-key.sh" switch "$T/new" 2>/dev/null || fail "switched to the same key twice"

bash "$HERE/rotate-key.sh" rollback >/dev/null
[ "$(cat "$DATA/id_ed25519.pub")" = "$A" ] || fail "rollback did not restore the old key"
[ "$(cat "$ETC/keys.env")" = "KEYS_ACCEPT=$B" ] || fail "rollback does not accept the new key"
bash "$HERE/rotate-key.sh" switch "$T/new" >/dev/null

bash "$HERE/rotate-key.sh" retire >/dev/null
[ "$(cat "$ETC/keys.env")" = "KEYS_ACCEPT=" ] || fail "retire still accepts a key"
[ ! -e "$DATA/id_ed25519.old" ] || fail "old secret key kept after retire"
[ "$(cat "$DATA/id_ed25519.pub")" = "$B" ] || fail "retire changed the signing key"
echo "rotate-key.sh: all checks passed"
