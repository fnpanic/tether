#!/usr/bin/env bash
# Black-box connection experiment: run the corporate proxy with one policy for N seconds and
# summarise what a product (by host-name match) tried. The VM's system proxy points at :8080.
#   corp_experiment.sh <name> <ports> <bypass-regex|-> <seconds> <host-match>
set -euo pipefail
name=$1 ports=$2 bypass=$3 secs=$4 match=$5
dir=/c/vms/exp/$name
mkdir -p "$dir" && cd "$dir" && : > corp_proxy_log.jsonl
args=(-s /c/install/remote-support/tools/bench/corp_proxy.py --listen-host 0.0.0.0 --listen-port "${PROXY_PORT:-8080}")
[ "$bypass" != "-" ] && args+=(--ignore-hosts "$bypass")
CORP_PORTS=$ports timeout "$secs" /c/vms/mitm-venv/Scripts/mitmdump "${args[@]}" > mitmdump.log 2>&1 || true
python - "$dir" "$match" <<'EOF'
import json, sys, collections
d, m = sys.argv[1], sys.argv[2]
ev = [json.loads(l) for l in open(d + '/corp_proxy_log.jsonl', encoding='utf-8') if l.strip()]
ev = [e for e in ev if m in str(e.get('host'))]
t0 = ev[0]['t'] if ev else 0
c = collections.Counter((e['event'], e.get('host'), e.get('port')) for e in ev)
for k, v in c.most_common(): print(f'{v:4d}', *k)
print('timeline:', ' '.join(f"{e['t']-t0:.0f}s:{e['event'][:4]}:{e.get('port')}" for e in ev[:30]))
EOF
grep -ac "does not trust\|unknown ca" mitmdump.log | sed 's/^/client TLS rejections: /'
