#!/usr/bin/env python3
"""turn-check.py <relay> [port]: is hbbs's TURN (UDP 21116) answering, and STUN still too?

An unauthenticated Allocate must come back as 401 with realm "tether" and a nonce (TURN is on);
a Binding request must come back with XOR-MAPPED-ADDRESS (STUN unchanged). No credentials needed.
"""
import os, socket, struct, sys

COOKIE = 0x2112A442


def request(method, attrs=b""):
    tid = os.urandom(12)
    return struct.pack("!HHI", method, len(attrs), COOKIE) + tid + attrs, tid


def attrs_of(msg):
    out, i = {}, 20
    while i + 4 <= len(msg):
        t, n = struct.unpack("!HH", msg[i:i + 4])
        out[t] = msg[i + 4:i + 4 + n]
        i += 4 + n + (-n % 4)
    return out


def ask(sock, addr, method, attrs=b""):
    m, tid = request(method, attrs)
    for _ in range(3):
        sock.sendto(m, addr)
        try:
            r, _ = sock.recvfrom(2048)
        except socket.timeout:
            continue
        if r[8:20] == tid:
            return struct.unpack("!H", r[:2])[0], attrs_of(r)
    return None, {}


def main():
    host = sys.argv[1]
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 21116
    addr = (socket.gethostbyname(host), port)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(1.5)
    ok = True
    t, a = ask(s, addr, 0x0001)
    if t == 0x0101 and 0x0020 in a:
        x = a[0x0020]
        p = struct.unpack("!H", x[2:4])[0] ^ (COOKIE >> 16)
        ip = socket.inet_ntoa(struct.pack("!I", struct.unpack("!I", x[4:8])[0] ^ COOKIE))
        print(f"STUN ok: mapped {ip}:{p}")
    else:
        ok = False
        print(f"STUN: no binding response ({t})")
    # Allocate, REQUESTED-TRANSPORT = UDP (17)
    t, a = ask(s, addr, 0x0003, struct.pack("!HHB3x", 0x0019, 4, 17))
    if t == 0x0113 and 0x0009 in a:
        code = a[0x0009][2] * 100 + a[0x0009][3]
        realm = a.get(0x0014, b"").decode(errors="replace")
        print(f"TURN ok: Allocate -> {code}, realm {realm!r}, nonce {'yes' if 0x0015 in a else 'no'}")
        ok = ok and code == 401 and realm == "tether"
    else:
        ok = False
        print(f"TURN: no Allocate error response ({t}) - TURN off or old hbbs")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
