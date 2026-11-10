#!/bin/bash
# Malware checks for release files, to be run by hand for every build before it is published
# (docs/release-checks.md; nothing else enforces it): Microsoft Defender scan, then VirusTotal
# (scripts/vtscan.py saves the reports under tools/release/virustotal/). Exit 1 when anything flags a
# file, 2 when a check gave no result (missing file, VirusTotal error or no finished analysis).
#   bash scripts/release-checks.sh dist/windows/Tether_<v>_x64-setup.exe dist/quick/TetherQS-<v>.exe ...
set -u
[ $# -gt 0 ] || { echo "usage: $0 FILE..." >&2; exit 2; }
R=$(cd "$(dirname "$0")/.." && pwd)
MP="/c/Program Files/Windows Defender/MpCmdRun.exe"
rc=0
for f in "$@"; do
  [ -f "$f" ] || { echo "missing: $f" >&2; rc=2; continue; }
  if [ -x "$MP" ]; then
    out=$("$MP" -Scan -ScanType 3 -File "$(cygpath -w "$(realpath "$f")")" 2>&1 | tail -1)
    echo "Defender: $out"
    echo "$out" | grep -q "found no threats" || rc=1
  else
    echo "Defender: not available here, skipped for $f"
  fi
done
python "$R/scripts/vtscan.py" "$@"
vt=$?
[ $vt -gt $rc ] && rc=$vt
[ $rc -eq 0 ] && echo "release checks: all clean" || echo "release checks: FAILED (rc $rc) - do not publish"
exit $rc
