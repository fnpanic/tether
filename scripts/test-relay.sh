#!/usr/bin/env bash
# Unit tests of the patched rustdesk-server (hbbs/hbbr drain, health, relay choice) and tether-api,
# run after scripts/build-relay.sh in the same build dir, then the backup relay failover end to end.
set -euo pipefail
OUT="${1:-$HOME/ha-build}"
source "$HOME/.cargo/env"
export CC_x86_64_unknown_linux_musl=musl-gcc
T=x86_64-unknown-linux-musl
(cd "$OUT/rustdesk-server" && cargo test --release --target $T --lib --bin hbbr)
# test-hooks: the test-only settings the tests use; then the tests of a production build alone.
(cd "$OUT/server/api" && cargo test --release --target $T --features test-hooks)
(cd "$OUT/server/api" && cargo test --release --target $T -- production_build)
bash "$(dirname "$0")/test-relay-backup.sh" "$OUT"
