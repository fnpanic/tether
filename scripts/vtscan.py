#!/usr/bin/env python3
"""VirusTotal check of release files (stdlib only). Part of every release, see docs/release-checks.md.

  python scripts/vtscan.py dist/windows/Tether_0.5.31_x64-setup.exe dist/quick/TetherQS-0.5.29.exe ...

Per file: look the SHA-256 up on VirusTotal; if unknown, upload it (files over 32 MB through an
upload URL) and wait for the analysis. The full report is saved as
tools/release/virustotal/<file name>/<sha256 prefix>.json plus a line in summary.md there.
Exit 1 when any engine reports the file as malicious or suspicious (names printed), 2 on errors.

API key: env VT_API_KEY, else the file C:\\vms\\virustotal\\api-key.txt (never printed or saved).
Uploading shares the file with VirusTotal's partners; release files are public anyway.
"""
import hashlib
import json
import os
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

API = "https://www.virustotal.com/api/v3"
KEY_FILE = Path(r"C:\vms\virustotal\api-key.txt")
OUT = Path(__file__).resolve().parent.parent / "tools" / "release" / "virustotal"
DIRECT_LIMIT = 32 * 1024 * 1024


def key() -> str:
    k = os.environ.get("VT_API_KEY", "").strip()
    if not k and KEY_FILE.exists():
        k = KEY_FILE.read_text(encoding="utf-8").strip()
    if not k:
        sys.exit(f"no VirusTotal API key: set VT_API_KEY or write it to {KEY_FILE}")
    return k


_last = [0.0]
GAP = 15.5  # free API: 4 requests a minute (500 a day): never send faster


def call(method: str, url: str, k: str, body: bytes | None = None, ctype: str | None = None) -> dict:
    wait = _last[0] + GAP - time.monotonic()
    if wait > 0:
        time.sleep(wait)
    _last[0] = time.monotonic()
    req =urllib.request.Request(url if url.startswith("http") else API + url, data=body, method=method)
    req.add_header("x-apikey", k)
    req.add_header("accept", "application/json")
    if ctype:
        req.add_header("content-type", ctype)
    for attempt in range(6):
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                return json.loads(r.read() or b"{}")
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return {}
            if e.code == 429 and attempt < 5:  # free API: 4 requests a minute
                time.sleep(20)
                continue
            raise RuntimeError(f"VirusTotal {method} {url}: HTTP {e.code} {e.read()[:200]!r}") from None
    return {}


def upload(path: Path, k: str) -> str:
    url = "/files"
    if path.stat().st_size > DIRECT_LIMIT:
        url = call("GET", "/files/upload_url", k)["data"]
    b = uuid.uuid4().hex
    body = (f"--{b}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{path.name}\"\r\n"
            "Content-Type: application/octet-stream\r\n\r\n").encode() + path.read_bytes() + f"\r\n--{b}--\r\n".encode()
    return call("POST", url, k, body, f"multipart/form-data; boundary={b}")["data"]["id"]


def verdict(stats: dict) -> str:
    bad = stats.get("malicious", 0) + stats.get("suspicious", 0)
    return "CLEAN" if bad == 0 else "FLAGGED"


def scan(path: Path, k: str) -> tuple[str, dict]:
    sha = hashlib.sha256(path.read_bytes()).hexdigest()
    rep = call("GET", f"/files/{sha}", k)
    if not rep:
        aid = upload(path, k)
        for _ in range(30):  # one status query a minute, up to 30 min (quota)
            time.sleep(45)
            a = call("GET", f"/analyses/{aid}", k)
            if a.get("data", {}).get("attributes", {}).get("status") == "completed":
                break
        for _ in range(10):  # the file object can lag behind the finished analysis
            rep = call("GET", f"/files/{sha}", k)
            if rep.get("data", {}).get("attributes", {}).get("last_analysis_stats"):
                break
            time.sleep(30)
    return sha, rep


def main(files: list[str]) -> int:
    if not files:
        print(__doc__)
        return 2
    k = key()
    worst = 0
    for f in files:
        p = Path(f)
        try:
            sha, rep = scan(p, k)
        except Exception as e:  # noqa: BLE001 - report and go on with the next file
            print(f"ERROR {p.name}: {e}")
            worst = max(worst, 2)
            continue
        attrs = rep.get("data", {}).get("attributes", {})
        stats = attrs.get("last_analysis_stats", {})
        hits = sorted(f"{eng}: {r.get('result')}" for eng, r in attrs.get("last_analysis_results", {}).items()
                      if r.get("category") in ("malicious", "suspicious"))
        d = OUT / p.name
        d.mkdir(parents=True, exist_ok=True)
        (d / f"{sha[:16]}.json").write_text(json.dumps(rep, indent=1, sort_keys=True), encoding="utf-8")
        v = verdict(stats)
        line = (f"| {time.strftime('%Y-%m-%d %H:%M')} | {p.name} | `{sha[:16]}` | {v} | "
                f"{stats.get('malicious', 0)} malicious, {stats.get('suspicious', 0)} suspicious, "
                f"{stats.get('undetected', 0)} undetected | {'; '.join(hits) or '-'} |")
        summary = OUT / "summary.md"
        if not summary.exists():
            summary.write_text("# VirusTotal results of release files\n\n| Checked | File | SHA-256 | Verdict | Engines | Detections |\n"
                               "| --- | --- | --- | --- | --- | --- |\n", encoding="utf-8")
        with summary.open("a", encoding="utf-8") as s:
            s.write(line + "\n")
        print(f"{v} {p.name} {sha[:16]} {stats}" + (f"\n  " + "\n  ".join(hits) if hits else ""))
        print(f"  https://www.virustotal.com/gui/file/{sha}")
        if v != "CLEAN":
            worst = max(worst, 1)
    return worst


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
