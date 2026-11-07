#!/bin/bash
# Build-runner job (user "tether"): the Mac build of Tether + TetherQS, signed with the self-signed
# "Tether Code Signing" certificate in a temporary keychain, checked against the installed app's
# designated requirement. Replacing /Applications/Tether.app is left to on-unlock.sh (swap.sh),
# which the runner starts once the screen is unlocked. Sent by submit.sh; see docs/mac-build-runner.md.
#
# work/: build.sh swap.sh src.tgz        secrets/: keys.env cs.p12 p12.pass
# $JOB_CACHE keeps src/ (with .tools/ and Cargo's target/) between builds, so builds are incremental.
set -uo pipefail
B=$JOB_CACHE
S=$JOB_SECRETS
log() { echo "[$(date +%H:%M:%S)] $*"; }
fail() { log "FAIL $*"; exit 1; }
log "start (job $JOB_ID)"
mkdir -p "$B/src" "$B/sign"
kc="$B/sign/tmp.keychain-db"
# A previous run killed before its trap ran: drop its keychain (its password is gone) and search-list entry.
if [ -e "$kc" ]; then
  security delete-keychain "$kc" 2>/dev/null || rm -f "$kc"
  log "removed a stale temporary keychain"
fi
rm -rf "$B/src/vendor" && tar -xzf "$JOB_WORK/src.tgz" -C "$B/src" || fail "untar"
rm -f "$JOB_WORK/src.tgz"
log "version $(grep -m1 '"version"' "$B/src/apps/desktop/src-tauri/tauri.conf.json")"

kp=$(openssl rand -hex 16)
orig=$(security list-keychains -d user | tr -d '"' | xargs)
cleanup() {
  # shellcheck disable=SC2086 # list of keychain paths
  security list-keychains -d user -s $orig
  security delete-keychain "$kc" 2>/dev/null
  rm -f "$B/sign/ls-test"
  log "temporary keychain deleted, search list: $(security list-keychains -d user | xargs)"
}
trap cleanup EXIT
# A keychain of our own, unlocked with its own random password: no login keychain, no GUI prompt,
# works with the screen locked. The partition list lets codesign use the key without asking.
security create-keychain -p "$kp" "$kc" &&
  security set-keychain-settings -lut 21600 "$kc" &&
  security unlock-keychain -p "$kp" "$kc" &&
  security import "$S/cs.p12" -k "$kc" -P "$(cat "$S/p12.pass")" -T /usr/bin/codesign >/dev/null &&
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kp" "$kc" >/dev/null ||
  fail "keychain import"
rm -f "$S/cs.p12" "$S/p12.pass" # the runner deletes secrets/ after the job anyway
log "certificate imported; p12 and password deleted: $(ls "$S")"
# shellcheck disable=SC2086
security list-keychains -d user -s "$kc" $orig
security find-identity -p codesigning "$kc"
cp /bin/ls "$B/sign/ls-test"
codesign -f -s "Tether Code Signing" --keychain "$kc" "$B/sign/ls-test" || fail "codesign refuses the certificate"
log "codesign test ok"

export HOMEBREW_NO_AUTO_UPDATE=1
brew install cmake ninja pkg-config || fail "brew"
# shellcheck source=/dev/null
. "$S/keys.env"
rm -f "$S/keys.env"
# CI=true: Tauri's dmg step skips its Finder AppleScript (needs Automation consent, fails without a GUI).
export CI=true PREBUILT_FRONTEND="$B/src/prebuilt/desktop-dist" PREBUILT_QS_FRONTEND="$B/src/prebuilt/quick-dist" \
  MAC_ARCH=arm64 APPLE_SIGNING_IDENTITY="Tether Code Signing" OUT="$JOB_WORK/out"
rm -rf "$OUT"
log "build"
(cd "$B/src" && bash scripts/build-macos.sh) || fail "build"

APP="$B/src/apps/desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Tether.app"
new=$(codesign -d -r- "$APP" 2>&1 | grep designated)
old=$(codesign -d -r- /Applications/Tether.app 2>&1 | grep designated)
log "requirement new: $new"
log "requirement old: $old"
[ -n "$new" ] && [ "$new" = "$old" ] || fail "requirement differs"
codesign --verify --deep --strict "$APP" || fail "codesign verify"
QAPP="$B/src/apps/quick/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/TetherQS.app"
qs=$(codesign -d -r- "$QAPP" 2>&1 | grep designated)
log "requirement QS: $qs"
echo "$qs" | grep -q 'certificate root = H"bcf53642c6c9f74356a3d12ae740bcc0eb59fb33"' &&
  codesign --verify --deep --strict "$QAPP" || fail "QS signature"
ls -l "$OUT"
trap - EXIT
cleanup

# Swap later (the running Tether is this Mac's remote access): a copy that later builds cannot touch.
rm -rf "$JOB_WORK/Tether.app" && ditto "$APP" "$JOB_WORK/Tether.app" || fail "copy app"
if [ "${TETHER_SWAP:-1}" = 1 ]; then
  printf '#!/bin/bash\nexec /bin/bash "$JOB_WORK/swap.sh" "$JOB_WORK/Tether.app"\n' >"$JOB_ON_UNLOCK"
  log "BUILD OK; Tether.app will be replaced when the screen is unlocked"
else
  log "BUILD OK (no swap requested)"
fi
