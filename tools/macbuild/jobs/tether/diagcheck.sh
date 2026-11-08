#!/bin/bash
# Diagnostics check on the Mac (build.sh with TETHER_DIAG_CHECK=1): the built Tether.app and
# TetherQS.app each write their diagnostics zip (`--diagnostics`) under a throw-away HOME (no real
# config or logs of this Mac are read), twice: the first run creates the config (ID, key pair,
# salt), then a planted config file with secrets and a log file are added. The zip must hold
# diagnostics.json, logs/ and config/, and no secret value. Zips and the report go to $OUT.
#   diagcheck.sh <Tether.app> <TetherQS.app> <out dir>
set -uo pipefail
APP=$1 QAPP=$2 OUT=$3
H=$(mktemp -d)
trap 'rm -rf "$H"' EXIT
fails=0
for which in tech qs; do
  if [ $which = tech ]; then bin="$APP/Contents/MacOS/tether"; else bin="$QAPP/Contents/MacOS/TetherQS"; fi
  [ -x "$bin" ] || bin=$(ls "$(dirname "$bin")"/* | head -1)
  HOME=$H "$bin" --diagnostics "$H/$which-0.zip" >/dev/null 2>&1
  for d in "$H"/Library/Preferences/*/; do
    printf 'password = "planted-secret-123"\n[options]\nrecording-upload-token = "planted-token-456"\n[socks]\nproxy = "proxy:3128"\npassword = "planted-proxy-pass"\n' >"$d/Planted.toml"
  done
  mkdir -p "$H/Library/Logs/Tether" "$H/Library/Logs/Tether-QS"
  for d in "$H"/Library/Logs/*/; do echo "[2026-11-08 00:00:00] ERROR diagcheck planted line" >"$d/diagcheck.log"; done
  HOME=$H "$bin" --diagnostics "$OUT/diag-$which.zip"
  "${JOB_PYTHON:-python3}" - "$OUT/diag-$which.zip" "$H" "$which" <<'PY' || fails=$((fails + 1))
import re, sys, zipfile, pathlib, json
z, home, which = zipfile.ZipFile(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
names = sorted(z.namelist())
blob = b"".join(z.read(n) for n in names).decode("latin1")
secrets = ["planted-secret-123", "planted-token-456", "planted-proxy-pass"]
for f in home.glob("Library/Preferences/*/*.toml"):
    if f.name == "Planted.toml":
        continue
    for k, v in re.findall(r'^\s*([A-Za-z0-9_.-]+)\s*=\s*[\'"]([^\'"]{6,})[\'"]', f.read_text(errors="replace"), re.M):
        if re.search("password|salt|key|token|secret|pin|otp|2fa", k, re.I) and v not in ("Y", "N"):
            secrets.append(v)
info = json.loads(z.read("diagnostics.json"))
leaked = [s[:4] + "…" for s in secrets if s in blob]
ok = "diagnostics.json" in names and any(n.startswith("logs/") for n in names) and any(n.startswith("config/") for n in names) and not leaked and "<redacted>" in blob and "diagcheck planted line" in blob
print(f"{'PASS' if ok else 'FAIL'}  macOS {which}: {len(names)} files {names}; {len(secrets)} secret values checked, leaked {leaked}; version {info.get('version')}, logDir {info.get('logDir')}")
sys.exit(0 if ok else 1)
PY
done
exit $fails
