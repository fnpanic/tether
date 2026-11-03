#!/usr/bin/env bash
# Static musl build of hbbs, hbbr (+ relay-harness) and tether-api, run inside WSL/Linux:
#   scripts/build-relay.sh [build-dir]    (default ~/ha-build)
# Copies vendor/rustdesk-server and server/api from this checkout into build-dir, builds there and
# collects the binaries in build-dir/bin.
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$HOME/ha-build}"
source "$HOME/.cargo/env"
export CC_x86_64_unknown_linux_musl=musl-gcc
T=x86_64-unknown-linux-musl
mkdir -p "$OUT/bin"
# db_v2.sqlite3 is the schema sqlx checks queries against at compile time: copy it, skip only -wal/-shm.
rsync -a --delete --exclude target --exclude .git --exclude '*.sqlite3-*' --exclude src/version.rs "$SRC/vendor/rustdesk-server/" "$OUT/rustdesk-server/"
mkdir -p "$OUT/server" "$OUT/crates"
rsync -a --delete --exclude target "$SRC/server/api/" "$OUT/server/api/"
rsync -a --delete --exclude target "$SRC/crates/tether-trust/" "$OUT/crates/tether-trust/"
(cd "$OUT/rustdesk-server" && cargo build --release --target $T --bin hbbs --bin hbbr --bin relay-harness)
(cd "$OUT/server/api" && cargo build --release --target $T)
for b in hbbs hbbr relay-harness; do install -m 0755 "$OUT/rustdesk-server/target/$T/release/$b" "$OUT/bin/"; done
install -m 0755 "$OUT/server/api/target/$T/release/tether-api" "$OUT/bin/"
ls -l "$OUT/bin"
