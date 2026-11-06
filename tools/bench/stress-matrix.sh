#!/bin/bash
# stress-matrix.sh <cdp-port> <out-prefix> <minutes-per-codec> <codec>...: for each codec, set it on the
# running viewer session (session_codec) and record stress-sampler output to <out-prefix>-<codec>.jsonl.
PORT=$1; PFX=$2; MIN=$3; shift 3
D=$(cd "$(dirname "$0")" && pwd)
for c in "$@"; do
  (cd "$D/../../apps/desktop" && node -e '
const { chromium } = require("playwright-core");
(async () => { const b = await chromium.connectOverCDP("http://127.0.0.1:" + process.argv[1]);
const p = b.contexts().flatMap(x => x.pages()).find(x => x.url().includes("/session/"));
const key = p.url().match(/session\/([0-9a-f]{32})/)[1];
console.log("codec", process.argv[2], await p.evaluate(([k, c]) => window.__TAURI_INTERNALS__.invoke("session_codec", { key: k, codec: c }).then(() => "set", e => "ERR " + e), [key, process.argv[2]]));
await b.close(); })();' "$PORT" "$c")
  sleep 5
  node "$D/stress-sampler.mjs" --port "$PORT" --seconds $((MIN * 60)) --every 250 --out "$PFX-$c.jsonl"
  python "$D/stress-report.py" "$PFX-$c.jsonl"
done
