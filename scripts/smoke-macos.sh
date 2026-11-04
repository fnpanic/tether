#!/usr/bin/env bash
# Start the built macOS apps once and check what a user would see: a window must appear, and no
# admin/password dialog (SecurityAgent, osascript "with administrator privileges") may open.
# Used by CI after scripts/build-macos.sh; screenshots and logs go to $OUT/smoke (default dist/smoke).
#
#   scripts/smoke-macos.sh [dist/macos]
set -uo pipefail
cd "$(dirname "$0")/.."
SRC=${1:-dist/macos}
OUT=${OUT:-dist/smoke}
WAIT=${WAIT:-20}
mkdir -p "$OUT"
work=$(mktemp -d)
tar -xzf "$SRC"/Tether_*_universal.app.tar.gz -C "$work"
ditto -x -k "$SRC"/TetherQS-*-universal.zip "$work"

# On-screen windows as "owner<TAB>title<TAB>layer"; owner names need no Screen Recording permission.
cat >"$work/windows.swift" <<'EOF'
import CoreGraphics
import Foundation
let list = (CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as NSArray?) as? [[String: Any]] ?? []
for w in list {
  let owner = w["kCGWindowOwnerName"] as? String ?? ""
  let name = w["kCGWindowName"] as? String ?? ""
  let layer = w["kCGWindowLayer"] as? Int ?? -1
  let b = w["kCGWindowBounds"] as? [String: Any] ?? [:]
  print("\(owner)\t\(name)\t\(layer)\t\(b["Width"] ?? 0)x\(b["Height"] ?? 0)")
}
EOF
swiftc -O -o "$work/windows" "$work/windows.swift" || { echo "error: swiftc failed"; exit 1; }

fail=0
# Signature: with a certificate (EXPECT_SIGNED=1) the designated requirement names it, so macOS keeps
# Screen Recording / Accessibility across updates; ad-hoc ("cdhash") loses them on every update.
for app in Tether.app TetherQS.app; do
  dr=$(codesign -d -r- "$work/$app" 2>&1 | grep designated || true)
  echo "== $app signature: $(codesign -dv "$work/$app" 2>&1 | grep -E '^Authority=|Signature=adhoc' | head -1) | $dr"
  if [ "${EXPECT_SIGNED:-}" = 1 ] && ! echo "$dr" | grep -q "certificate"; then
    echo "FAIL: $app is not signed with the certificate"; fail=1
  fi
done
check() { # app-name binary-name
  local app=$1 bin=$2
  local log="$OUT/$bin.out"
  echo "== $app"
  RUST_LOG=info "$work/$app/Contents/MacOS/$bin" >"$log" 2>&1 &
  local pid=$!
  sleep "$WAIT"
  "$work/windows" >"$OUT/$bin.windows.txt"
  screencapture -x "$OUT/$bin.png" 2>/dev/null || true
  ps -axo pid,comm,args >"$OUT/$bin.ps.txt"
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "FAIL: $bin exited"; tail -30 "$log"; fail=1; return
  fi
  # Our process's own windows of normal size (layer 0 = normal window level).
  if awk -F'\t' -v o="$bin" 'tolower($1)==tolower(o) && $3==0' "$OUT/$bin.windows.txt" | grep -q .; then
    echo "ok: window shown: $(awk -F'\t' -v o="$bin" 'tolower($1)==tolower(o) && $3==0 {print $2" "$4}' "$OUT/$bin.windows.txt" | head -1)"
  else
    echo "FAIL: no window of $bin on screen"; grep -i -F "$bin" "$OUT/$bin.windows.txt"; fail=1
  fi
  if grep -E "SecurityAgent|osascript|authopen|Installer" "$OUT/$bin.ps.txt" | grep -v grep; then
    echo "FAIL: an admin/password prompt is open"; fail=1
  else
    echo "ok: no admin prompt"
  fi
  grep -E "^(SecurityAgent|UserNotificationCenter|universalAccessAuthWarn|System Settings|tccd)" "$OUT/$bin.windows.txt" || true
  pkill -P "$pid" 2>/dev/null; kill "$pid" 2>/dev/null; sleep 2; kill -9 "$pid" 2>/dev/null
  pkill -x "$bin" 2>/dev/null || true
}
check Tether.app tether
check TetherQS.app TetherQS
find ~/Library/Logs -newer "$work/windows.swift" -name "*.log" -exec cp {} "$OUT/" \; 2>/dev/null || true
rm -rf "$work"
[ $fail = 0 ] && echo "smoke: all ok" || echo "smoke: FAILED"
exit $fail
