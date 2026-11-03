# Video performance on GPU-less hosts (window drag)

Symptom reported: dragging a window on a Hyper-V VM without a GPU (watched in the Hyper-V console)
made the Tether viewer sluggish, and the remote picture stopped updating while the window visibly
moved on the VM.

## Setup

- **Host:** Hyper-V VM `tether-bench`, Windows 11 LTSC, 4 vCPU, 8 GB, **no GPU** (Microsoft Hyper-V
  Video, 1920×1080), TetherQS for the standard user `customer`. Software encoding only.
- **Viewer:** technician app (release build, profile `perf-viewer`) on the Hyper-V host PC, RTX 3080
  (WebCodecs hardware decode).
- **Network:** both sides go through the production relay `tether-desk.duckdns.org`.
  - *corp-proxy*: the VM's egress is blocked except for the TLS-inspecting mitmproxy
    (`tools/bench/corp_proxy.py`).
  - *direct*: temporarily allowed egress and no proxy (`C:\vms\perf\direct.ps1`, restored afterwards).
  - The proxy's bulk throughput was measured from inside the VM: 107 Mbit/s through the proxy and
    151 Mbit/s direct. So the proxy is not a bandwidth limit for a 1–2 Mbit/s stream.
- **Workload:** `C:\vms\perf\drag.ps1` (in the VM's interactive session) opens a 900×600 document-like
  window and drags it by its title bar with real mouse input (SendInput), ±500 px on a 2 s period
  for 26 s, with `tools/bench/probe.ps1` visible top-left. It also records the host's CPU use.

## Method: `apps/desktop/e2e/bench-drag.mjs`

The viewer window stays off the visible desktop, so nothing is measured optically. Instead, a hook in
the viewer's webview (over CDP) wraps `CanvasRenderingContext2D.drawImage` and does two things:

- It timestamps every picture drawn into the session canvas. The **max draw gap** (longest time
  without a new picture) is the freeze metric. During a drag every encoded frame carries new content.
- It decodes the probe barcode (VM clock) from each decoded `VideoFrame`. It reads only a 2-line rect
  with an asynchronous `clone().copyTo()`. **Latency** = viewer clock at draw − (VM clock + offset),
  with the offset from `clock_offset.py`. This is "glass-to-canvas": everything except the viewer's
  own display refresh (~8 ms on average at 60 Hz).

The first version of the hook read the canvas synchronously (`getImageData`) on every frame. That
stalled the viewer's GPU pipeline (4 instead of 19 drawn frames/s), so those runs are marked
`invalid` in `tools/bench/results/drag.json`. About 15 % of the frames during a drag are too blurred
to decode the barcode. They still count as drawn frames, but they give no latency sample.

Host-side encoder numbers come from the new `fluxrt encode:` log line (Tether builds only, every 5 s):
fps, average/max encode time, bitrate and key frames.

Run it with:

    node e2e/bench-drag.mjs --tech <tether.exe> --label <name> [--codec auto|av1|vp9|vp8] [--seconds 20]

## Findings

1. **Key-frame refresh storm (viewer).** `VideoPipeline` dropped every delta and asked for a key
   frame once as soon as `decodeQueueSize > 3`. Packets arrive in bursts through relays and proxies,
   so the queue briefly passed 3 even with a fast decoder. On the host every refresh restarts the
   whole video service: capturer, encoder and a large key frame (~80 ms plus transfer). With VP8 the
   host log showed `switch to refresh` every ~0.5 s during the drag, and the picture froze between
   key frames. If a refresh got lost, the picture stayed frozen for good, because nothing asked again.
2. **Codec choice on GPU-less hosts.** Measured during the drag at the same ~1.4 Mbit/s target:

   | Codec (software) | Host fps | Encode avg | Result |
   | --- | --- | --- | --- |
   | AV1 (aom, auto) | 18–21 | 20–25 ms | smooth |
   | VP9 | 3–4 | 42–54 ms | slow and bit-hungry |
   | VP8 | 3 | 12–16 ms | fast encode, but rate control drops frames: screen content needs far more bits |

   AV1 is the right automatic choice. Upstream Auto, however, falls back to **VP8 on hosts with
   ≤ 4 GB RAM**, which is typical for customer VMs and is the worst case here.
3. **Periodic key frames hurt.** An experiment with a key frame every 300 frames for software encoders
   raised the max draw gap from 134–200 ms to 238–265 ms: each AV1 intra frame is a burst at this
   bitrate. Recovery is better done on demand (item 1), so this experiment was dropped.
4. **`video.codec` policy was not enforced.** The policy wrote `codec-preference` into the global
   options, but the viewer reads it from the per-peer config.
5. **Host CPU is not the limit:** 24–42 % total of 4 vCPU during the drag, and TetherQS uses about
   0.5–0.7 cores. AV1 fps is bounded by serial capture + encode (~45 ms per frame).

## Changes

| Where | Change |
| --- | --- |
| `apps/desktop/src/lib/video.ts` | `KeyRecovery`: ask for a key frame at once and again every second until one arrives; resync only when the decoder is more than 24 frames behind (`MAX_DECODE_QUEUE`); a fresh decoder waits for a key frame and asks for one; stall stats (max gap, key requests, dropped frames) shown in the statistics overlay and refreshed even while frozen; console warnings for every wait. |
| `vendor/rustdesk/libs/scrap/src/common/codec.rs` (`tauri` feature) | Keep AV1 on ≤ 4 GB hosts (`keep_av1_on_low_memory`); the AV1 speed test still guards slow hosts. A forced (policy) `codec-preference` overrides the per-peer one. |
| `vendor/rustdesk/src/server/video_service.rs` (`tauri`) | `fluxrt encode:` host log line every 5 s. |
| `apps/desktop/src-tauri/src/policy.rs` | `codec-preference` goes to the display-setting layers (`OVERWRITE_/DEFAULT_DISPLAY_SETTINGS`), so `video.codec` takes effect. |

Unchanged on purpose: the 60 fps ceiling (the encoder loop paces itself by capture + encode time;
AV1 here runs at ~19 fps); upstream's delay-based QoS.

## Results

TBD

## Result (0.4.0, 2026-11-04)

Same VM and corporate proxy, technician 0.4.0 viewer and TetherQS 0.4.0 host (AV1, 4 encoder
threads), 20 s window drag, `apps/desktop/e2e/bench-drag.mjs`:

| Build | Remote frames/s (drag) | Longest gap between drawn pictures | Latency p95 (drag) |
| --- | --- | --- | --- |
| 0.3.0 as reported (refresh storm) | 3–4 | 1.6 s, or frozen until a lucky key frame | 1.2 s |
| 0.3.0, meter v2 | 16–19 | ~500 ms (barcode) | ~90 ms |
| **0.4.0** | **20 / 23** | **90 ms** (0 gaps > 100 ms) | **88 / 55 ms** |

The barcode-based "freeze" figure in `drag.json` (446–486 ms) counts frames whose probe barcode
is too blurred to decode during motion (81 of ~460 frames) as missing; the canvas draw gaps
show the picture never stopped. Viewer stats: 0 key-frame requests, 0 dropped frames.

## AnyDesk

Not measured in the window-drag scenario. The idle LAN comparison is in `docs/benchmarks.md`.
