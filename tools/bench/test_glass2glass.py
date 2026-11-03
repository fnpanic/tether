"""Self-check: render synthetic barcodes like probe.html and make sure the meter decodes them."""

import numpy as np

from glass2glass import BITS, find_barcodes, signed_diff


def render(img, x, y, cell, h, ms):
    v = ms & 0xFFFFFFFF
    by = [v & 255, (v >> 8) & 255, (v >> 16) & 255, (v >> 24) & 255]
    s = (sum(by) + 0x5A) & 255
    bits = [(v >> i) & 1 for i in range(31, -1, -1)] + [(s >> i) & 1 for i in range(7, -1, -1)]
    img[y : y + h, x : x + cell] = (255, 0, 0)
    for i, b in enumerate(bits):
        img[y : y + h, x + cell * (i + 1) : x + cell * (i + 2)] = (0, 0, 0) if b else (255, 255, 255)
    img[y : y + h, x + cell * (BITS + 1) : x + cell * (BITS + 2)] = (0, 255, 0)


def test_live_and_scaled_mirror():
    img = np.full((600, 1400, 3), 128, np.uint8)
    render(img, 10, 10, 20, 64, 1_762_123_456_789)  # live
    render(img, 300, 300, 12, 38, 1_762_123_456_731)  # mirror inside a viewer, 58 ms older
    codes = sorted(find_barcodes(img), reverse=True)
    assert len(codes) == 2
    assert signed_diff(codes[0][1], codes[1][1]) == 58


def test_mirror_larger_than_live_still_pairs_by_time():
    import glass2glass as g

    img = np.full((700, 2400, 3), 128, np.uint8)
    render(img, 10, 10, 20, 64, 5_000)  # live
    render(img, 1100, 300, 25, 80, 4_950)  # viewer shows it scaled UP
    codes = g.find_barcodes(img)
    vals = sorted({v for _, v in codes}, key=lambda v: g.signed_diff(v, codes[0][1]), reverse=True)
    assert g.signed_diff(vals[0], vals[1]) == 50


def test_rejects_corrupt_checksum():
    img = np.full((120, 1000, 3), 128, np.uint8)
    render(img, 10, 10, 20, 64, 123456)
    img[12:70, 10 + 20 * 5 : 10 + 20 * 6] = 255 - img[40, 10 + 20 * 5]  # flip one data bit
    assert find_barcodes(img) == []


if __name__ == "__main__":
    test_live_and_scaled_mirror()
    test_mirror_larger_than_live_still_pairs_by_time()
    test_rejects_corrupt_checksum()
    print("ok")


def test_clock_latency_uses_remote_offset():
    from glass2glass import clock_latency
    # Remote (VM) clock runs 400 ms ahead: remote - local = +400 -> offset arg = -400.
    # The viewer shows remote time 10_400 when local capture time is 10_030: 30 ms latency.
    assert clock_latency(10_030, 10_400, -400) == 30
