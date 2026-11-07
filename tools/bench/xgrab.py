"""Grab an X display (Xvfb) where a viewer runs, for the picture-quality bench (docs/performance.md).
Used for the AnyDesk Linux viewer on Xvfb in WSL: nothing appears on the Windows desktop.

    python3 xgrab.py shot out.png                      whole screen as PNG
    python3 xgrab.py sample --seconds 30 --out x.jsonl  decode the Date.now() barcode of quality.html /
                                                       stress.html as fast as it can (small rect only);
                                                       same JSON lines as stress-sampler.mjs
    ... sample --serve <port>                          also answer udp-stamp.mjs (on Windows) with the
                                                       latest value and its age, so Windows can time-stamp
                                                       it with its own clock (the WSL clock can be a second off)
Display from $DISPLAY.
"""
import argparse
import json
import time

import numpy as np
from Xlib import X, display


def grab(d, x, y, w, h):
    raw = d.screen().root.get_image(x, y, w, h, X.ZPixmap, 0xFFFFFFFF).data
    a = np.frombuffer(raw, np.uint8).reshape(h, w, 4)
    return a[:, :, 2::-1]  # BGRX -> RGB


def find(img):
    """Locate the barcode: a red cell, 40 bit cells, a green cell, all the same width."""
    H, W, _ = img.shape
    r, g, b = (img[:, :, i].astype(int) for i in range(3))
    red = (r > 150) & (g < 90) & (b < 90)
    grn = (g > 150) & (r < 90) & (b < 90)
    for j in range(2, H, 3):
        i = 0
        while i < W - 50:
            if not red[j, i]:
                i += 1
                continue
            e = i
            while e < W and red[j, e]:
                e += 1
            gx = e
            while gx < W and not grn[j, gx]:
                gx += 1
            if gx >= W:
                break
            cell = (gx - i) / 41
            if cell >= 4 and abs(cell - (e - i)) <= 3:
                loc = (i, j, cell)
                if decode(img, loc) is not None:
                    return loc
            i = e
    return None


def decode(img, loc):
    rx, y, cell = loc
    bits = []
    for k in range(40):
        p = img[y, int(round(rx + cell * (k + 1) + cell / 2))].astype(int)
        bits.append(1 if (p[0] * 3 + p[1] * 6 + p[2]) / 10 < 128 else 0)
    v = 0
    for k in range(32):
        v = v * 2 + bits[k]
    s = 0
    for k in range(32, 40):
        s = s * 2 + bits[k]
    by = [v & 255, (v >> 8) & 255, (v >> 16) & 255, (v >> 24) & 255]
    return v if (sum(by) + 0x5A) & 255 == s else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["shot", "sample"])
    ap.add_argument("out", nargs="?")
    ap.add_argument("--out", dest="out2")
    ap.add_argument("--seconds", type=float, default=30)
    ap.add_argument("--every", type=float, default=10, help="ms between samples")
    ap.add_argument("--serve", type=int)
    a = ap.parse_args()
    d = display.Display()
    s = d.screen()
    W, H = s.width_in_pixels, s.height_in_pixels
    if a.cmd == "shot":
        from PIL import Image
        Image.fromarray(np.ascontiguousarray(grab(d, 0, 0, W, H))).save(a.out)
        return
    out = open(a.out2 or a.out, "a")
    udp, last = None, None
    if a.serve:
        import socket
        udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        udp.bind(("0.0.0.0", a.serve))
        udp.setblocking(False)
    loc = None
    end = time.time() + a.seconds
    n = ok = 0
    while time.time() < end:
        if loc is None:
            loc = find(grab(d, 0, 0, min(W, 1600), min(H, 600)))
            if loc is None:
                out.write(json.dumps({"t": int(time.time() * 1000), "state": "no barcode"}) + "\n")
                time.sleep(0.5)
                continue
            rx, y, cell = loc
            x0, w = int(rx), int(cell * 42) + 2
        img = grab(d, x0, y, w, 1)
        t = int(time.time() * 1000)
        v = decode(img, (rx - x0, 0, cell))
        if v is not None and (last is None or last[0] != v):
            last = (v, time.monotonic())
        while udp:
            try:
                _, peer = udp.recvfrom(16)
            except BlockingIOError:
                break
            if last:
                udp.sendto(f"{last[0]} {(time.monotonic() - last[1]) * 1000:.1f}".encode(), peer)
        rec = {"t": t, "state": ""}
        if v is not None:
            hi = (t // 2**32) * 2**32
            host = hi + v
            if host - t > 2**31:
                host -= 2**32
            if t - host > 2**31:
                host += 2**32
            rec = {"t": t, "host": host, "lat": t - host, "phase": (host // 60000) % 4, "state": ""}
            ok += 1
        out.write(json.dumps(rec) + "\n")
        n += 1
        time.sleep(a.every / 1000)
    print(f"samples {n}, decoded {ok}")


if __name__ == "__main__":
    main()
