#!/usr/bin/env bash
# Rebuild binaries in WSL, the image, and load it into the kind cluster (run from Git Bash).
#   deploy/kind/rebuild-image.sh <tag>
set -euo pipefail
TAG="${1:?tag}"
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
WIN_HERE="$(cygpath -w "$HERE" 2>/dev/null || echo "$HERE")"
wsl -d Ubuntu-22.04 -e bash -lc "bash \"\$(wslpath '$WIN_HERE')/scripts/build-relay.sh\" > ~/ha-build/build.log 2>&1 || { tail -30 ~/ha-build/build.log; exit 1; }; mkdir -p \"\$(wslpath '$WIN_HERE')/dist/relay-image\" && cp ~/ha-build/bin/* \"\$(wslpath '$WIN_HERE')/dist/relay-image/\""
docker build -q -f "$HERE/server/Dockerfile.static" -t "tether-relay:$TAG" "$HERE/dist/relay-image"
# kind load docker-image fails on Docker Desktop's containerd store; an archive works.
tar="$HERE/dist/relay-image.tar"
docker save --platform linux/amd64 "tether-relay:$TAG" -o "$tar"
kind.exe load image-archive "$tar" --name tether-ha
