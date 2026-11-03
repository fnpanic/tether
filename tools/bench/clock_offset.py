"""Measure the clock offset between two hosts (e.g. WSL and Windows) with UDP time echoes.

    WSL:      python3 clock_offset.py serve 0.0.0.0 47999
    Windows:  python clock_offset.py probe <wsl-ip> 47999
Prints offset_ms = remote_clock - local_clock, taken from the lowest-RTT sample (NTP-style).
"""

import socket
import struct
import sys
import time


def serve(host, port):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind((host, int(port)))
    while True:
        data, addr = s.recvfrom(64)
        s.sendto(data + struct.pack("!d", time.time() * 1000), addr)


def probe(host, port, n=200):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(1)
    best = None
    for i in range(n):
        t0 = time.time() * 1000
        s.sendto(struct.pack("!I", i), (host, int(port)))
        try:
            data, _ = s.recvfrom(64)
        except socket.timeout:
            continue
        t1 = time.time() * 1000
        (remote,) = struct.unpack("!d", data[4:12])
        rtt = t1 - t0
        off = remote - (t0 + t1) / 2
        if best is None or rtt < best[0]:
            best = (rtt, off)
        time.sleep(0.005)
    if best is None:
        sys.exit("no replies")
    print(f"{best[1]:.3f} rtt={best[0]:.3f}")


if __name__ == "__main__":
    {"serve": serve, "probe": probe}[sys.argv[1]](*sys.argv[2:4])
