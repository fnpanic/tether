"""Barcode probe drawn by a Linux (WSLg) window, same encoding as probe.html.

Used to calibrate the extra display delay WSLg adds to any Linux window (for example a Linux
remote-desktop viewer) so cross-platform comparisons can be corrected.  Run inside WSL:

    python3 wsl_probe.py
"""

import time
import tkinter as tk

CELL, H, PAD, BITS = 20, 64, 8, 40


def bits_for(ms):
    v = ms & 0xFFFFFFFF
    by = [v & 255, (v >> 8) & 255, (v >> 16) & 255, (v >> 24) & 255]
    s = (sum(by) + 0x5A) & 255
    return [(v >> i) & 1 for i in range(31, -1, -1)] + [(s >> i) & 1 for i in range(7, -1, -1)]


root = tk.Tk()
root.title("WSLg probe")
root.geometry(f"{PAD * 2 + CELL * (BITS + 2)}x{H + PAD * 2}+40+520")
cv = tk.Canvas(root, width=PAD * 2 + CELL * (BITS + 2), height=H + PAD * 2, bg="#808080", highlightthickness=0)
cv.pack()
cv.create_rectangle(PAD, PAD, PAD + CELL, PAD + H, fill="#ff0000", outline="")
cv.create_rectangle(PAD + CELL * (BITS + 1), PAD, PAD + CELL * (BITS + 2), PAD + H, fill="#00ff00", outline="")
cells = [cv.create_rectangle(PAD + CELL * (i + 1), PAD, PAD + CELL * (i + 2), PAD + H, outline="") for i in range(BITS)]


def tick():
    for c, b in zip(cells, bits_for(int(time.time() * 1000))):
        cv.itemconfigure(c, fill="#000000" if b else "#ffffff")
    root.after(4, tick)


tick()
root.mainloop()
