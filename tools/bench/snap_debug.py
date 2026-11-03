"""One screenshot: list decoded barcodes and red/green marker candidates; save the image."""

import sys

import mss
import numpy as np
from PIL import Image

from glass2glass import components, find_barcodes

with mss.MSS() if hasattr(mss, "MSS") else mss.mss() as sct:
    mon = sct.monitors[1]
    img = np.asarray(sct.grab(mon))[..., :3][..., ::-1]
print("screen", img.shape)
r, g, b = (img[..., i].astype(np.int16) for i in range(3))
print("red comps", components((r > 170) & (g < 90) & (b < 90))[:10])
print("green comps", components((g > 170) & (r < 100) & (b < 100))[:10])
print("barcodes", find_barcodes(img))
if len(sys.argv) > 1:
    Image.fromarray(img).save(sys.argv[1])
