#!/bin/bash
# Submit the Tether Mac build (HEAD of this checkout) to the Mac build runner as user "tether".
# Run from Git Bash on the PC:  bash tools/macbuild/jobs/tether/submit.sh [--no-swap] [--pack-only] [--diag-upload]
# Builds both UIs here (npm is blocked on the Mac) under the PC's build lock, packs src.tgz, and
# sends build.sh + swap.sh with keys.env and the signing p12 as per-job secrets (deleted on the Mac
# after the job). Prints the job id; then: python tools/macbuild/macbuild.py wait --user tether <id>
# Env: TETHER_KEYS_ENV (default C:/vms/mac59/keys.env), MACSIGN (default C:/vms/macsign),
# MAC_ARCH=universal (release build: Intel + Apple silicon; default arm64 for test builds),
# TETHER_DIAG_CHECK=1 (also run diagcheck.sh: both apps' diagnostics zip, checked for secrets).
# --diag-upload: instead of the build, diagupload.sh (debug builds of both apps upload diagnostics
# to a local test relay with a throw-away admin key; keys.env only, no signing secrets).
set -euo pipefail
R=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
J=$R/tools/macbuild/jobs/tether
KEYS=${TETHER_KEYS_ENV:-/c/vms/mac59/keys.env}
SIGN=${MACSIGN:-/c/vms/macsign}
SWAP=1 EXTRA=() JOB=build
for a in "$@"; do
  case $a in
    --no-swap) SWAP=0 ;;
    --pack-only) EXTRA+=(--pack-only) ;;
    --diag-upload) JOB=diagupload ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done
for f in "$KEYS" $([ $JOB = build ] && echo "$SIGN/cs.p12 $SIGN/p12.pass"); do [ -f "$f" ] || { echo "missing $f" >&2; exit 1; }; done
LOCK=/c/vms/night/buildlock.sh
run() { if [ -f "$LOCK" ]; then bash "$LOCK" "$@"; else "$@"; fi; }
if [ -z "${SKIP_UI_BUILD:-}" ]; then
  run bash -c "cd '$R/apps/desktop' && npm ci --silent && npm run build && cd '$R/apps/quick' && npm ci --silent && npm run build" >&2
fi
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
mkdir -p "$W/work/prebuilt"
# LF whatever the Windows checkout has (a CRLF swap.sh failed on the Mac with "set: -\r", 2026-11-09).
for f in build.sh swap.sh diagcheck.sh diagupload.sh; do tr -d '\r' <"$J/$f" >"$W/work/$f"; done
# git archive keeps LF and file modes; the prebuilt UIs are appended under prebuilt/.
git -C "$R" -c core.autocrlf=false archive --format=tar -o "$W/src.tar" HEAD
cp -r "$R/apps/desktop/dist" "$W/work/prebuilt/desktop-dist"
cp -r "$R/apps/quick/dist" "$W/work/prebuilt/quick-dist"
tar -rf "$W/src.tar" -C "$W/work" prebuilt
rm -rf "$W/work/prebuilt"
gzip -c "$W/src.tar" >"$W/work/src.tgz"
echo "src.tgz: $(du -h "$W/work/src.tgz" | cut -f1) of $(git -C "$R" rev-parse --short HEAD)" >&2
# Windows paths for the native Python client.
w() { cygpath -w "$1"; }
if [ $JOB = diagupload ]; then
  python "$(w "$R/tools/macbuild/macbuild.py")" submit --user tether --work "$(w "$W/work")" \
    --secret "$(w "$KEYS")=keys.env" --timeout 14400 "${EXTRA[@]}" -- bash diagupload.sh
  exit
fi
python "$(w "$R/tools/macbuild/macbuild.py")" submit --user tether --work "$(w "$W/work")" \
  --secret "$(w "$KEYS")=keys.env" --secret "$(w "$SIGN/cs.p12")=cs.p12" --secret "$(w "$SIGN/p12.pass")=p12.pass" \
  --env TETHER_SWAP=$SWAP --env TETHER_DIAG_CHECK=${TETHER_DIAG_CHECK:-0} --env MAC_ARCH=${MAC_ARCH:-arm64} --timeout 14400 --artifact 'out/*' "${EXTRA[@]}" -- bash build.sh
