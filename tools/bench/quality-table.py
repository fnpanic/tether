"""Summarise quality-run.sh results: one row per viewer/setting (label prefix before "-<n>").

    python quality-table.py <outdir> [prefix ...]
Static runs: quality of the first picture (:02) and the settled one (:40), bytes the VM sent for the
page switch (the second with the page) and in the rest of the 30 s window, VM CPU.
Scroll runs: quality of the still left half during the scroll, sharpness of the scrolling panel,
latency/fps from the barcode samples, VM bytes and CPU.
"""
import glob
import json
import os
import statistics as st
import sys
from collections import defaultdict

d = sys.argv[1]
want = sys.argv[2:]
rows = [json.loads(l) for l in open(os.path.join(d, "quality.jsonl"))]
by = defaultdict(lambda: defaultdict(list))
for r in rows:
    run, shot = r["label"].rsplit("-", 1)
    by[run.rsplit("-", 1)[0]][shot].append(r)
mean = lambda xs: st.mean(xs) if xs else float("nan")


def perf(prefix):
    out = defaultdict(list)
    for f in glob.glob(os.path.join(d, f"{prefix}-*.vmperf.json")):
        try:
            p = json.loads(open(f, encoding="utf-8-sig").read())
        except ValueError:
            continue
        s = p["sent_kB_per_s"]
        out["kB_switch"].append(max(s[:4]))
        out["kB_after"].append(sum(s) - max(s[:4]))
        out["kbps"].append(p["sent_kbps"])
        out["cpu"].append(p["cpu_total"])
        out["cpu_host"].append(p["cpu_proc"].get("TetherQS", 0) + p["cpu_proc"].get("AnyDesk", 0))
    return {k: mean(v) for k, v in out.items()}, len(out["kbps"])


def lat(prefix):
    xs, fps = [], []
    for f in glob.glob(os.path.join(d, f"{prefix}-*.lat.jsonl")):
        rs = [json.loads(l) for l in open(f) if '"lat"' in l]
        if not rs:
            continue
        xs += [r["lat"] for r in rs]
        span = (rs[-1]["t"] - rs[0]["t"]) / 1000
        fps.append(len({r["host"] for r in rs}) / span if span else 0)
    if not xs:
        return None
    xs.sort()
    return {"lat_med": xs[len(xs) // 2], "lat_p95": xs[int(len(xs) * 0.95)], "distinct_fps": mean(fps)}


for prefix in sorted(by):
    if want and prefix not in want:
        continue
    shots = by[prefix]
    pf, n = perf(prefix)
    print(f"== {prefix} ({n} runs)")
    for shot in sorted(shots):
        rs = shots[shot]
        a = {k: mean([r["all"][k] for r in rs]) for k in rs[0]["all"]}
        left = {k: mean([r["left"][k] for r in rs]) for k in rs[0]["left"]}
        txt = {k: mean([r["text"][k] for r in rs]) for k in ("psnr_y", "sharp", "fringe")}
        col = {k: mean([r["color"][k] for r in rs]) for k in ("psnr_y", "psnr_cb", "psnr_cr", "fringe")}
        ph = mean([r["photo"]["psnr_y"] for r in rs])
        sc = mean([r["scroll"]["sharp"] for r in rs])
        print(f"  {shot:8} all: Y {a['psnr_y']:.2f} Cb {a['psnr_cb']:.2f} Cr {a['psnr_cr']:.2f} dB, SSIM {a['ssim_y']:.4f}, MS-SSIM {a['msssim_y']:.4f}"
              f" | left: Y {left['psnr_y']:.2f} SSIM {left['ssim_y']:.4f} | text: Y {txt['psnr_y']:.2f} sharp {txt['sharp']:.3f} fringe {txt['fringe']:.1f}"
              f" | colour text: Y {col['psnr_y']:.2f} Cb {col['psnr_cb']:.2f} fringe {col['fringe']:.1f} | photo Y {ph:.2f} | scroll panel sharp {sc:.3f}")
    if pf:
        print(f"  VM: {pf['kbps']:.0f} kbit/s avg, switch second {pf['kB_switch']:.0f} kB, rest {pf['kB_after']:.0f} kB, CPU {pf['cpu']:.1f} % (host app {pf['cpu_host']:.1f} %)")
    lt = lat(prefix)
    if lt:
        print(f"  latency median {lt['lat_med']} ms, p95 {lt['lat_p95']} ms (incl. clock offset), distinct frames/s {lt['distinct_fps']:.1f}")
