"""Optical glass-to-glass latency meter for any remote-desktop product.

The host screen shows tools/bench/probe.html (Date.now() as a barcode). A viewer window on the
same desktop shows a remote copy of that screen. Every screenshot therefore contains the *live*
barcode and an *older* copy inside the viewer; their difference is the full pipeline latency
(capture -> encode -> network -> decode -> present), measured with one clock and no access to the
product's internals.

    python glass2glass.py --seconds 20 --label tether --out results.json
"""

import argparse
import json
import statistics
import time

import mss
import numpy as np
from scipy import ndimage

BITS = 40


def components(mask):
    lab, n = ndimage.label(mask)
    out = []
    for sl in ndimage.find_objects(lab):
        if sl is None:
            continue
        y0, y1, x0, x1 = sl[0].start, sl[0].stop, sl[1].start, sl[1].stop
        w, h = x1 - x0, y1 - y0
        if w >= 3 and h >= 6 and h > w * 1.4:
            out.append((x0, y0, x1, y1))
    return out


def decode_bits(bits):
    v = 0
    for b in bits[:32]:
        v = (v << 1) | b
    s = 0
    for b in bits[32:]:
        s = (s << 1) | b
    by = [v & 255, (v >> 8) & 255, (v >> 16) & 255, (v >> 24) & 255]
    return v if (sum(by) + 0x5A) & 255 == s else None


def find_barcodes(img):
    """img: HxWx3 uint8 RGB. Returns [(span_px, value)] for every valid barcode."""
    r, g, b = img[..., 0].astype(np.int16), img[..., 1].astype(np.int16), img[..., 2].astype(np.int16)
    # Ratio-based so dimmed, colour-shifted or HDR-mapped screens still match.
    reds = components((r > 60) & (r > g * 3) & (r > b * 3))
    greens = components((g > 60) & (g > r * 3) & (g > b * 3))
    lum = (r * 3 + g * 6 + b) // 10
    found = []
    for (rx0, ry0, rx1, ry1) in reds:
        cell = rx1 - rx0
        cy = (ry0 + ry1) // 2
        best = None
        for (gx0, gy0, gx1, gy1) in greens:
            if gx0 <= rx1 or abs((gy0 + gy1) // 2 - cy) > (ry1 - ry0) * 0.4:
                continue
            span = gx0 - rx1
            if 0.7 < span / (cell * BITS) < 1.4 and (best is None or span < best[0]):
                best = (span, gx0)
        if not best:
            continue
        span = best[0]
        vals = []
        for i in range(BITS):
            x = int(rx1 + (i + 0.5) * span / BITS)
            vals.append(lum[max(cy - 2, 0) : cy + 3, max(x - 1, 0) : x + 2].mean())
        lo, hi = min(vals), max(vals)
        if hi - lo < 25:
            continue
        mid = (lo + hi) / 2
        bits = [1 if v < mid else 0 for v in vals]
        v = decode_bits(bits)
        if v is not None:
            found.append((span, v))
    return found


def clock_latency(t_cap, shown, offset):
    """Latency of a barcode drawn with another host's clock. offset = −(remote − local)."""
    return signed_diff(t_cap, (shown + int(round(offset))) & 0xFFFFFFFF)


def signed_diff(a, b):
    d = (a - b) & 0xFFFFFFFF
    return d - (1 << 32) if d >= (1 << 31) else d


def locate(sct, mon):
    """Find barcode rows once on a full frame; later frames only capture those bands (fast)."""
    shot = np.asarray(sct.grab(mon))[..., :3][..., ::-1]
    r, g, b = (shot[..., i].astype(np.int16) for i in range(3))
    boxes = components((r > 60) & (r > g * 3) & (r > b * 3))
    bands = []
    for (x0, y0, x1, y1) in boxes:
        h = y1 - y0
        span = (x1 - x0) * (BITS + 2)
        bands.append({
            "left": mon["left"] + max(x0 - 4, 0),
            "top": mon["top"] + max(y0 - 2, 0),
            "width": min(int(span * 1.5) + 8, mon["width"] - max(x0 - 4, 0)),
            "height": h + 4,
        })
    return bands


def grab_codes(sct, bands):
    codes = []
    for band in bands:
        img = np.asarray(sct.grab(band))[..., :3][..., ::-1]
        codes.extend(find_barcodes(img))
    return codes


class DxgiSource:
    """DXGI Desktop Duplication (GPU path, refresh-rate capture). Yields only *new* frames."""

    def __init__(self):
        import dxcam

        self.cam = dxcam.create(output_color="BGRA")
        self.region = None

    def full(self):
        for _ in range(200):
            f = self.cam.grab()
            if f is not None:
                return f[..., :3][..., ::-1]
            time.sleep(0.01)
        raise RuntimeError("no frame from DXGI")

    def focus(self, bands):
        if not bands:
            self.region = None
            return
        l = min(b["left"] for b in bands)
        t = min(b["top"] for b in bands)
        r = max(b["left"] + b["width"] for b in bands)
        bt = max(b["top"] + b["height"] for b in bands)
        self.region = (l, t, r, bt)

    def codes(self):
        f = self.cam.grab(region=self.region) if self.region else self.cam.grab()
        if f is None:
            return None  # nothing changed on screen since the last frame
        return find_barcodes(f[..., :3][..., ::-1])


def bands_from(shot):
    r, g, b = (shot[..., i].astype(np.int16) for i in range(3))
    out = []
    H, W = shot.shape[:2]
    for (x0, y0, x1, y1) in components((r > 60) & (r > g * 3) & (r > b * 3)):
        band = shot[max(y0 - 2, 0) : y1 + 2, max(x0 - 4, 0) : min(W, x0 + (x1 - x0) * (BITS + 4))]
        if find_barcodes(band):  # keep only red markers that start a real barcode
            out.append({"left": max(x0 - 4, 0), "top": max(y0 - 2, 0), "width": min(W, x0 + (x1 - x0) * (BITS + 4)) - max(x0 - 4, 0), "height": min(H, y1 + 2) - max(y0 - 2, 0)})
    return out


def run(seconds, monitor_idx, pair="mirror", offset=0.0):
    lat, live_lag, mirror_values, frames, valid = [], [], set(), 0, 0
    try:
        src = DxgiSource()
    except Exception:
        src = None
    if src is None:
        return run_gdi(seconds, monitor_idx, pair, offset)
    src.focus(bands_from(src.full()))
    end = time.time() + seconds
    misses = 0
    while time.time() < end:
        t_cap = int(time.time() * 1000) & 0xFFFFFFFF
        found = src.codes()
        if found is None:
            time.sleep(0.0005)
            continue
        frames += 1
        codes = sorted(found, reverse=True)
        if len(codes) < 2:
            misses += 1
            if misses % 60 == 0:
                src.focus(bands_from(src.full()))  # windows moved: re-scan
        if True:
            if not codes:
                continue
            if pair == "clock":
                # Host clock is known (offset measured): one viewer barcode is enough.
                v = codes[0][1]
                d = clock_latency(t_cap, v, offset)
                if 0 <= d < 5000:
                    lat.append(d)
                    mirror_values.add(v)
                    valid += 1
                continue
            if pair == "newest":
                if len(codes) < 2:
                    continue
                vals = sorted((v for _, v in codes), key=lambda v: signed_diff(v, codes[0][1]), reverse=True)
                live, other = vals[0], vals[1]
                d = signed_diff(live, (other + int(round(offset))) & 0xFFFFFFFF)
                if -1000 < d < 5000:
                    lat.append(d)
                    mirror_values.add(other)
                    valid += 1
                continue
            vals = sorted({v for _, v in codes}, key=lambda v: signed_diff(v, codes[0][1]), reverse=True)
            live = vals[0]
            live_lag.append(signed_diff(t_cap, live))
            if len(vals) < 2:
                continue
            d = signed_diff(live, vals[1])
            if 0 <= d < 5000:
                lat.append(d)
                mirror_values.add(vals[1])
                valid += 1
    return lat, live_lag, mirror_values, frames, valid


def run_gdi(seconds, monitor_idx, pair="mirror", offset=0.0):
    lat, live_lag, mirror_values, frames, valid = [], [], set(), 0, 0
    with (mss.MSS() if hasattr(mss, "MSS") else mss.mss()) as sct:
        mon = sct.monitors[monitor_idx]
        bands = locate(sct, mon)
        end = time.time() + seconds
        while time.time() < end:
            t_cap = int(time.time() * 1000) & 0xFFFFFFFF
            frames += 1
            codes = sorted(grab_codes(sct, bands), reverse=True)
            if not codes and frames % 30 == 0:
                bands = locate(sct, mon)  # windows moved: re-scan
            if not codes:
                continue
            if pair == "newest":
                # Two independent probes (different clocks/display paths): newest vs the other one.
                if len(codes) < 2:
                    continue
                vals = sorted((v for _, v in codes), key=lambda v: signed_diff(v, codes[0][1]), reverse=True)
                live, other = vals[0], vals[1]
                d = signed_diff(live, (other + int(round(offset))) & 0xFFFFFFFF)
                if -1000 < d < 5000:
                    lat.append(d)
                    mirror_values.add(other)
                    valid += 1
                continue
            # Order by time, not size (a viewer may show the remote screen at or above 1:1):
            # newest = live probe, next newest = the copy one hop through the viewer.
            vals = sorted({v for _, v in codes}, key=lambda v: signed_diff(v, codes[0][1]), reverse=True)
            live = vals[0]
            live_lag.append(signed_diff(t_cap, live))
            if len(vals) < 2:
                continue
            mirrors = vals[1:]
            d = signed_diff(live, mirrors[0])
            if 0 <= d < 5000:
                lat.append(d)
                mirror_values.add(mirrors[0])
                valid += 1
    return lat, live_lag, mirror_values, frames, valid


def summary(xs):
    if not xs:
        return None
    xs = sorted(xs)
    return {
        "n": len(xs),
        "median": statistics.median(xs),
        "mean": round(statistics.fmean(xs), 1),
        "p10": xs[int(len(xs) * 0.10)],
        "p95": xs[min(len(xs) - 1, int(len(xs) * 0.95))],
        "min": xs[0],
        "max": xs[-1],
        "stdev": round(statistics.pstdev(xs), 1),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seconds", type=float, default=20)
    ap.add_argument("--monitor", type=int, default=1)
    ap.add_argument("--label", default="run")
    ap.add_argument("--out")
    ap.add_argument("--subtract", type=float, default=0.0, help="calibrated viewer-path overhead to remove (ms)")
    ap.add_argument("--pair", choices=["mirror", "newest", "clock"], default="mirror")
    ap.add_argument("--offset", type=float, default=0.0, help="ms added to the second probe's clock (remote - local offset, negated)")
    a = ap.parse_args()

    lat, lag, mv, frames, valid = run(a.seconds, a.monitor, a.pair, a.offset)
    res = {
        "label": a.label,
        "seconds": a.seconds,
        "capture_fps": round(frames / a.seconds, 1),
        "valid_samples": valid,
        "distinct_remote_frames_per_s": round(len(mv) / a.seconds, 1),
        "latency_ms": summary(lat),
        "latency_corrected_ms": summary([x - a.subtract for x in lat]) if a.subtract else None,
        "screen_to_capture_ms": summary(lag),
    }
    print(json.dumps(res, indent=2))
    if a.out:
        try:
            with open(a.out) as f:
                allr = json.load(f)
        except (FileNotFoundError, json.JSONDecodeError):
            allr = []
        allr.append(res)
        with open(a.out, "w") as f:
            json.dump(allr, f, indent=2)


if __name__ == "__main__":
    main()
