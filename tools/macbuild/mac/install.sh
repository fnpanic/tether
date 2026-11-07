#!/bin/bash
# Installs (or updates) the build runner for this macOS user: no sudo, no GUI. See docs/mac-build-runner.md.
#
#   bash install.sh                      first install, typed once in Terminal: registers every
#                                        <user>.pub next to this script (admin.pub is required)
#   bash install.sh --update             replace runner.py only (sent as an admin job by
#                                        macbuild.py update-runner; the runner restarts when idle)
set -euo pipefail
SRC=$(cd "$(dirname "$0")" && pwd)
R=$HOME/build-runner
PY=/usr/bin/python3
LABEL=local.build-runner
P=$HOME/Library/LaunchAgents/$LABEL.plist

"$PY" -c 'import sys; assert sys.version_info >= (3, 9), sys.version' ||
  { echo "needs /usr/bin/python3 3.9+ (Xcode Command Line Tools)"; exit 1; }
command -v ssh-keygen >/dev/null || { echo "ssh-keygen missing"; exit 1; }
mkdir -p "$R"
chmod 700 "$R"
if [ "$SRC" != "$R" ]; then
  cp "$SRC/runner.py" "$R/runner.py.new" && mv "$R/runner.py.new" "$R/runner.py"
  cp "$SRC/install.sh" "$R/install.sh.new" && mv "$R/install.sh.new" "$R/install.sh"
fi
if [ "${1:-}" = --update ]; then
  echo "runner.py updated ($(shasum -a 256 "$R/runner.py" | cut -c1-16)); the runner restarts when no job runs"
  exit 0
fi

for f in "$SRC"/*.pub; do
  [ -e "$f" ] || continue
  u=$(basename "$f" .pub)
  if [ "$u" = admin ]; then "$PY" "$R/runner.py" add-user admin "$(cat "$f")" --no-sandbox
  else "$PY" "$R/runner.py" add-user "$u" "$(cat "$f")"; fi
done
grep -q '^admin ' "$R/keys/allowed_signers" 2>/dev/null || { echo "no admin key: put admin.pub next to install.sh"; exit 1; }

mkdir -p "$HOME/Library/LaunchAgents"
cat >"$P.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$PY</string><string>$R/runner.py</string><string>serve</string></array>
  <key>EnvironmentVariables</key><dict><key>BUILD_RUNNER_ROOT</key><string>$R</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>StandardOutPath</key><string>$R/runner.log</string>
  <key>StandardErrorPath</key><string>$R/runner.log</string>
</dict></plist>
EOF
plutil -lint "$P.new" >/dev/null
mv "$P.new" "$P"
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
sleep 1
launchctl bootstrap "gui/$(id -u)" "$P"
sleep 3
launchctl print "gui/$(id -u)/$LABEL" | grep -E '^\s*(state|pid) =' || true
tail -3 "$R/runner.log" 2>/dev/null || true
echo "build runner installed: $R (users: $(cut -d' ' -f1 "$R/keys/allowed_signers" | sort -u | xargs))"
