# Bench tools

Measure any remote-desktop product **optically**, with no access to its internals, on a single
machine.

| File | What it does |
| --- | --- |
| `probe.html` | Host-screen content. Draws `Date.now()` as a checksummed barcode every frame and logs input events with timestamps. `?mode=scroll` adds a busy scrolling-text workload. |
| `glass2glass.py` | Screenshots the desktop as fast as it can. In each screenshot it decodes the **live** barcode and the **mirrored** copy shown inside the viewer window. Latency = live − mirror from the same screenshot, so one clock and one capture instant are used: no clock sync, and the capture delay cancels out. Also reports distinct remote frames per second. |
| `wsl_probe.py` | The same barcode drawn by a Linux window under WSLg (WSL clock). Used to calibrate how much later a WSLg window presents than a native Windows window. |
| `clock_offset.py` | NTP-style UDP echo that measures the WSL ↔ Windows clock offset, keeping the lowest-RTT sample. |
| `test_glass2glass.py` | Self-test with synthetic barcodes (scaled mirror, corrupt checksum). |
| `../../apps/desktop/e2e/session.e2e.mjs` | Functional E2E for Tether: login, first frame, mouse, keyboard, input latency, 64 MB file transfer with hash check, E2E encryption flag. |

## Comparing against another product on one machine

The other product cannot connect to itself, so its viewer runs in **WSL (WSLg)** while its host
runs on Windows. WSLg presents windows through an RDP-based compositor, which adds delay that a
native viewer would not have. To keep the comparison fair:

1. `clock_offset.py`: WSL clock − Windows clock (sub-millisecond).
2. Calibration: run `probe.html` (Windows) and `wsl_probe.py` (WSLg) side by side, then
   `glass2glass.py --pair newest --offset <−offset>`. The median is the **extra presentation delay
   of a WSLg window** relative to a native one.
3. Measure the product with its viewer in WSLg, then subtract the calibration median
   (`--subtract`).

Both raw and corrected numbers are reported. Tether's viewer runs natively on Windows and needs no
correction.

## Running

```powershell
py -3 -m venv tools/bench/.venv; tools/bench/.venv/Scripts/pip install mss numpy scipy
tools/bench/.venv/Scripts/python tools/bench/glass2glass.py --seconds 20 --label tether --out tools/bench/results/latency.json
```
