# Benchmarks

Measured on 2026-11-03 on one Windows 11 machine with an RTX 3080 and a 2560×1440 display at 59 Hz.
Host and viewer were two Tether profiles on the same machine, connected P2P through a self-hosted
relay in WSL. Method: [tools/bench/README.md](../tools/bench/README.md) (optical glass-to-glass
measurement through DXGI capture, with one clock and one capture instant).

## Tether glass-to-glass latency

"Glass-to-glass" covers everything from the host's screen changing to the viewer's screen showing
it: host display refresh, capture, encode, network, decode and viewer present. Results are quantized
to the 16.7 ms display frame, so the mean is the most precise single number.

| Scenario | Mean | p10 | Median | p95 | Max |
| --- | --- | --- | --- | --- | --- |
| Idle desktop, Balanced | 30.2 ms | 17 | 33 | 35 | 51 |
| Scrolling text, Balanced | 25.7 ms | 16 | 33 | 34 | 51 |
| Scrolling text, Speed | **24.0 ms** | 16 | 17 | 34 | 50 |

Other session figures from the stats overlay:
- 57 fps delivered at full 2560×1440 resolution;
- WebCodecs hardware decode of 0.7 ms per frame;
- encoder HEVC via NVENC, zero-copy (VRAM);
- about 2.3 Mbit/s for a mostly static desktop with a moving barcode.

## Functional end-to-end (apps/desktop/e2e/session.e2e.mjs)

| Check | Result |
| --- | --- |
| Host registers with relay (pinned key) | pass |
| Password login → first frame | pass |
| Remote mouse click reaches host | pass (24 ms) |
| Remote keyboard ("tether") | pass (median 3 ms) |
| Input latency, 40 pointer moves | median 12 ms, p95 19 ms |
| 64 MB file upload, SHA-256 verified | pass (4.0 s, 16 MB/s) |
| Session reports end-to-end encryption | pass |

## What FluxRT tuning changed (measured)

| Change | Effect |
| --- | --- |
| Zero-copy VRAM encode (D3D11 texture → NVENC, `vram` feature) instead of the GPU→CPU→GPU RAM path | Host capture+encode dropped from 33.5 ms to under one frame. Delivered fps went from 30 to 57. |
| 60 fps default ceiling, sent explicitly at session start and after quality changes (upstream sends a custom fps only in "custom" quality) | Removes the fixed 30 fps cap |
| QoS LAN fast path: go straight to the viewer's cap on clean low-latency links (`video_qos.rs`, `tauri` feature) | No slow 15 → 60 fps ramp-up |
| Draw decoded frames at the full coded size (`renderSize`), working around WebView2 HEVC reporting a half-size visibleRect | Full-resolution text instead of a quarter of the pixels |
| Remote cursor shape used as the local CSS cursor; pointer moves coalesced per animation frame | Pointer feels local, with no cursor lag |

Mean latency went from about 41 ms (RAM path, 30 fps) to about 24–30 ms.

## Tether vs AnyDesk (same host, same path, same meter)

**Setup:**
- **Host:** a Hyper-V VM (Windows 11 Enterprise LTSC evaluation, 4 vCPU, 8 GB, **no GPU**, 1920×1080) running AnyDesk 9.8 and Tether.
- **Viewer:** this PC, AnyDesk for Windows and Tether.
- **Probe:** `tools/bench/probe.ps1` inside the VM draws the VM's clock.
- **Clock offset:** measured NTP-style over UDP (`clocksrv.ps1` / `clock_offset.py`); about 705 ms.
- **Latency:** capture time minus (barcode shown in the viewer + offset), which is absolute glass-to-glass latency.
- **Network:** both products went P2P over the Hyper-V switch. Tether's relay ran on this PC.

Each product was measured in several independent 20 s runs (one 10 s run for AnyDesk).

| Product | Run | Mean | Median | p10 | p95 | Max | Remote fps | Samples |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **Tether** (Balanced) | 1 | 33.8 ms | 32 | 21 | 54 | 79 | 28.0 | 892 |
| | 2 | 40.7 ms | 39 | 26 | 62 | 86 | 28.1 | 918 |
| | 3 | 39.2 ms | 38 | 25 | 61 | 88 | 28.1 | 1069 |
| AnyDesk 9.8 (default) | 1 (10 s) | 53.7 ms | 53 | — | 83 | 102 | 20.4 | 2306 |
| | 2 | 45.8 ms | 44 | 26 | 76 | 99 | 20.4 | 913 |
| | 3 | 53.3 ms | 51 | 29 | 88 | 122 | 20.4 | 1691 |
| | 4 | 55.5 ms | 55 | 33 | 85 | 102 | 17.4 | 2111 |

**Summary (mean of runs):**

| | Tether | AnyDesk | Difference |
| --- | --- | --- | --- |
| Glass-to-glass latency, mean | **37.9 ms** | 52.1 ms | Tether **27 % lower** |
| p95 latency | **59 ms** | 83 ms | Tether **29 % lower** |
| Delivered frames per second | **28.1** | 19.7 | Tether **43 % more** |

The ranges don't overlap: Tether's worst run (40.7 ms) is faster than AnyDesk's best (45.8 ms). Without a
GPU in the VM, both products encode in software here, so these numbers are higher than Tether's 24–30 ms on
real hardware (above).

Caveats:
- Runs were taken on a PC that was partly in use; each run was checked for valid samples and kept only if
  the viewer window stayed visible.
- Re-run with `node apps/desktop/e2e/bench-vm.mjs` while the PC is idle. If AnyDesk asks for the VM
  password, a person enters it (the harness never types into windows).

## Earlier single-machine attempt

Before the Hyper-V VM was available, a single-machine comparison was tried and abandoned:
- Windows Sandbox and Hyper-V are unavailable.
- AnyDesk cannot connect to itself.
- WSLg (used for a Linux AnyDesk viewer) repaints Linux windows only about every 5 s in this
  setup, so any number taken that way would measure WSLg, not AnyDesk.

Ready-to-run tooling for a two-machine comparison:
- `apps/desktop/e2e/bench-anydesk.mjs` (single machine with WSL viewer);
- the camera method in [fluxrt-spec.md](fluxrt-spec.md);
- `glass2glass.py` run on the viewer machine with `--pair newest --offset <clock offset>` from
  `clock_offset.py`.
