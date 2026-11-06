"""Summarize stress-sampler.mjs output per phase (and per hour for long runs).

    python stress-report.py run.jsonl [--hourly]
lat includes the clock offset between viewer and host: compare phases against idle ("+idle").
A stall is a span in which the decoded host time did not advance while the viewer clock did.
"""
import json, statistics, sys
from collections import defaultdict

NAMES = ["idle", "scroll", "photo", "noise"]

def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(len(xs) * p))] if xs else None

def main(path, hourly=False):
    rows = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()]
    if not rows:
        print("no samples"); return
    t0, t1 = rows[0]["t"], rows[-1]["t"]
    print(f"{path}: {len(rows)} samples over {(t1 - t0) / 60000:.1f} min")
    errs = [r for r in rows if "err" in r]
    states = defaultdict(int)
    for r in rows:
        if r.get("state"):
            states[r["state"]] += 1
    print(f"  errors {len(errs)}, non-empty states {dict(states)}")
    by = defaultdict(list)
    for r in rows:
        if "lat" in r:
            by[r["phase"]].append(r["lat"])
    idle = statistics.median(by[0]) if by[0] else None
    # stalls: consecutive decoded samples whose host time is unchanged
    stalls, cur = [], None
    prev = None
    for r in rows:
        if "host" not in r:
            continue
        if prev and r["host"] == prev["host"]:
            cur = cur or prev["t"]
        elif cur is not None:
            stalls.append((prev["t"] - cur, NAMES[prev["phase"]]))
            cur = None
        prev = r
    print(f"  {'phase':7} {'n':>6} {'median':>8} {'p95':>8} {'max':>8} {'+idle med':>10} {'+idle p95':>10}")
    for ph in range(4):
        xs = by.get(ph, [])
        if not xs:
            continue
        med, p95 = statistics.median(xs), pct(xs, 0.95)
        rel = lambda v: f"{v - idle:+10.0f}" if idle is not None else f"{'-':>10}"
        print(f"  {NAMES[ph]:7} {len(xs):6d} {med:8.0f} {p95:8.0f} {max(xs):8.0f} {rel(med)} {rel(p95)}")
    long = sorted((s for s in stalls if s[0] >= 500), reverse=True)
    print(f"  stalls >= 0.5 s: {len(long)}; longest: {long[:5]}")
    if hourly:
        hours = defaultdict(list)
        for r in rows:
            if "lat" in r:
                hours[int((r["t"] - t0) / 3600000)].append(r["lat"])
        for h in sorted(hours):
            xs = hours[h]
            print(f"  hour {h}: n {len(xs)}, median {statistics.median(xs):.0f}, p95 {pct(xs, 0.95):.0f}")
        dec = sum(1 for r in rows if "lat" in r)
        print(f"  decoded {dec}/{len(rows)} ({100 * dec / len(rows):.1f} %)")

if __name__ == "__main__":
    main(sys.argv[1], "--hourly" in sys.argv)
