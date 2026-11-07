"""Picture quality of a remote-desktop viewer against the host's own screenshot (tools/bench/quality.html).

    python quality-measure.py <ref.png> <shot.png> --label NAME [--out results.jsonl] [--crops DIR]

The shot may be larger than the reference (a viewer window with its frame and toolbar): the
1920x1080 picture is located by template matching at 1:1 (scaled viewers are rejected, not
silently resampled). Metrics on BT.601 full-range YCbCr, per region and over the scored area
(below the 88 px barcode bar, above the taskbar line y=1032):
  psnr_y / psnr_cb / psnr_cr   dB against the reference
  ssim_y                       SSIM (Gaussian window, sigma 1.5) on luma
  msssim_y                     5-scale MS-SSIM on luma (Wang et al. weights)
  sharp                        Laplacian variance relative to the reference (text regions; 1.0 = as sharp)
  fringe                       mean |dCb|+|dCr| on the reference's edge pixels (colour fringing, levels)
"""
import argparse
import json
import os
import sys

import cv2
import numpy as np
from skimage.metrics import structural_similarity

REGIONS = {
    "text": (0, 88, 640, 496),
    "color": (640, 88, 640, 496),
    "lines": (1280, 88, 640, 496),
    "ui": (0, 584, 640, 448),
    "photo": (640, 584, 640, 448),
    "grad": (1280, 584, 640, 248),
    "scroll": (1280, 832, 640, 200),
}
# The host taskbar (y >= 1032, its clock changes) can show above the kiosk window: never scored.
SCORED = (0, 88, 1920, 944)
LEFT = (0, 88, 1280, 944)  # text, color, ui, photo: still in ?mode=scroll as well
# Viewer overlays that are not part of the remote picture (AnyDesk's free-licence watermark): the
# reference is pasted there in every shot, whichever viewer, so no viewer is scored on them.
MASKS = [(1540, 1000, 380, 32)]
TEXTY = ("text", "color", "ui", "scroll")


def locate(ref, shot):
    """Crop the reference-sized picture out of `shot` (1:1)."""
    H, W = ref.shape[:2]
    if shot.shape[:2] == (H, W):
        return shot
    if shot.shape[0] < H or shot.shape[1] < W:
        sys.exit(f"shot {shot.shape[1]}x{shot.shape[0]} is smaller than the reference: scaled viewer?")
    # The lines panel is unique and sharp: match it, then derive the picture's origin.
    x, y, w, h = REGIONS["lines"]
    tpl = cv2.cvtColor(ref[y:y + h, x:x + w], cv2.COLOR_BGR2GRAY)
    res = cv2.matchTemplate(cv2.cvtColor(shot, cv2.COLOR_BGR2GRAY), tpl, cv2.TM_CCOEFF_NORMED)
    _, score, _, (mx, my) = cv2.minMaxLoc(res)
    ox, oy = mx - x, my - y
    if score < 0.6 or ox < 0 or oy < 0 or oy + H > shot.shape[0] or ox + W > shot.shape[1]:
        sys.exit(f"reference not found 1:1 in the shot (score {score:.2f} at {ox},{oy})")
    return shot[oy:oy + H, ox:ox + W]


def ycc(img):
    y, cr, cb = cv2.split(cv2.cvtColor(img, cv2.COLOR_BGR2YCrCb).astype(np.float64))
    return y, cb, cr


def psnr(a, b):
    mse = np.mean((a - b) ** 2)
    return 99.0 if mse == 0 else 10 * np.log10(255 ** 2 / mse)


def ssim(a, b):
    return structural_similarity(a, b, data_range=255, gaussian_weights=True, sigma=1.5, use_sample_covariance=False)


def msssim(a, b):
    weights = [0.0448, 0.2856, 0.3001, 0.2363, 0.1333]
    vals = []
    for i, wt in enumerate(weights):
        s, cs = _ssim_cs(a, b)
        vals.append(s if i == len(weights) - 1 else cs)
        a = cv2.pyrDown(a)
        b = cv2.pyrDown(b)
    vals = np.maximum(np.array(vals), 1e-6)
    return float(np.prod(vals ** np.array(weights)))


def _ssim_cs(a, b):
    C1, C2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    g = lambda x: cv2.GaussianBlur(x, (11, 11), 1.5)
    ma, mb = g(a), g(b)
    va, vb, cov = g(a * a) - ma * ma, g(b * b) - mb * mb, g(a * b) - ma * mb
    cs = (2 * cov + C2) / (va + vb + C2)
    s = ((2 * ma * mb + C1) / (ma * ma + mb * mb + C1)) * cs
    return float(s.mean()), float(cs.mean())


def lapvar(y):
    return float(cv2.Laplacian(y, cv2.CV_64F).var())


def metrics(ref, img, name):
    ry, rcb, rcr = ycc(ref)
    ty, tcb, tcr = ycc(img)
    m = {
        "psnr_y": psnr(ry, ty), "psnr_cb": psnr(rcb, tcb), "psnr_cr": psnr(rcr, tcr),
        "ssim_y": ssim(ry, ty), "msssim_y": msssim(ry, ty),
    }
    if name in TEXTY or name == "all":
        m["sharp"] = lapvar(ty) / max(lapvar(ry), 1e-9)
        edges = np.abs(cv2.Laplacian(ry, cv2.CV_64F)) > 20
        m["fringe"] = float((np.abs(tcb - rcb) + np.abs(tcr - rcr))[edges].mean()) if edges.any() else 0.0
    return {k: round(float(v), 4) for k, v in m.items()}


def crops(ref, img, label, d):
    os.makedirs(d, exist_ok=True)
    # Small, telling windows, zoomed 3x nearest-neighbour: reference on top, viewer below.
    for name, (x, y, w, h) in {"text": (0, 88, 320, 110), "color": (640, 88, 320, 160), "lines": (1280, 88, 320, 200), "ui": (0, 584, 320, 160)}.items():
        a, b = ref[y:y + h, x:x + w], img[y:y + h, x:x + w]
        pair = np.vstack([a, np.full((4, w, 3), 255, np.uint8), b])
        cv2.imwrite(os.path.join(d, f"{label}-{name}.png"), cv2.resize(pair, None, fx=3, fy=3, interpolation=cv2.INTER_NEAREST))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("ref")
    ap.add_argument("shot")
    ap.add_argument("--label", required=True)
    ap.add_argument("--out")
    ap.add_argument("--crops")
    a = ap.parse_args()
    ref = cv2.imread(a.ref, cv2.IMREAD_COLOR)
    img = locate(ref, cv2.imread(a.shot, cv2.IMREAD_COLOR))
    img = img.copy()
    for x, y, w, h in MASKS:
        img[y:y + h, x:x + w] = ref[y:y + h, x:x + w]
    x, y, w, h = SCORED
    r = {"label": a.label, "all": metrics(ref[y:y + h, x:x + w], img[y:y + h, x:x + w], "all")}
    x, y, w, h = LEFT
    r["left"] = metrics(ref[y:y + h, x:x + w], img[y:y + h, x:x + w], "all")
    for name, (x, y, w, h) in REGIONS.items():
        r[name] = metrics(ref[y:y + h, x:x + w], img[y:y + h, x:x + w], name)
    print(json.dumps(r))
    if a.out:
        with open(a.out, "a") as f:
            f.write(json.dumps(r) + "\n")
    if a.crops:
        crops(ref, img, a.label, a.crops)


if __name__ == "__main__":
    main()
