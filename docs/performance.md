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

## Result (0.4.0, 2026-11-04)

Same VM and corporate proxy, technician 0.4.0 viewer and TetherQS 0.4.0 host (AV1, 4 encoder
threads), 20 s window drag, `apps/desktop/e2e/bench-drag.mjs`:

| Build | Remote frames/s (drag) | Longest gap between drawn pictures | Latency p95 (drag) |
| --- | --- | --- | --- |
| 0.3.0 as reported (refresh storm) | 3–4 | 1.6 s, or frozen until a lucky key frame | 1.2 s |
| 0.3.0, meter v2 | 16–19 | ~500 ms (barcode) | ~90 ms |
| **0.4.0** | **20 / 23** | **90 ms** (0 gaps > 100 ms) | **88 / 55 ms** |
| **0.4.0, proxy verified** (VM egress only via the TLS-inspecting proxy) | **20.1 / 22.9** | **99 / 79 ms** (0 gaps > 100 ms) | **66 / 59 ms** |

The barcode-based "freeze" figure in `drag.json` (446–486 ms) counts frames whose probe barcode
is too blurred to decode during motion (81 of ~460 frames) as missing; the canvas draw gaps
show the picture never stopped. Viewer stats: 0 key-frame requests, 0 dropped frames.

## AnyDesk (black-box, same scenario)

`node e2e/bench-vm.mjs --only <tether|anydesk> --drag` measures both viewers the same way: the
optical meter (`tools/bench/glass2glass.py`) reads the VM's clock barcode from the viewer window on
this PC while `drag.ps1` drags a window in the VM. Same VM, same relay path through the corporate
proxy; AnyDesk is exempt from TLS inspection (it refuses the corporate CA, as real deployments
must allow for), so for the fair row Tether was exempt as well. 20 s each, 2026-11-04.

| Viewer (host in the VM) | Latency mean | Latency p95 | Distinct remote frames/s | Longest freeze |
| --- | --- | --- | --- | --- |
| AnyDesk 9.8.3 | 62.4 ms | 95 ms | 11.2 | 509 ms |
| Tether 0.4.0 (inspection exempt, like AnyDesk) | 72.2 ms | 98 ms | 12.6 | 507 ms |
| Tether 0.4.0 (TLS-inspected by the proxy) | 74.3 ms | 102 ms | 14.7 | 563 ms |

Optical frames/freeze figures are conservative for both (motion blurs the barcode, so some
pictures don't count; the canvas draw-gap meter above shows Tether's picture never paused more than
~99 ms). Result: **on par** — same p95 latency and worst gap, Tether delivers slightly more
pictures, AnyDesk is ~10 ms lower on average. The idle LAN comparison is in `docs/benchmarks.md`.

# Bands across Edge pages and videos on GPU-less hosts (capture)

Symptom reported (0.5.4): in a session to the Hyper-V VM, a white/grey horizontal band flickered
across Edge's animated "Welcome to Edge" page and across videos. In the user's screen recording of
the viewer, 93 of 164 frames have one.

## Cause: Desktop Duplication hands out half-drawn windows

The bands are flat `#F3F3F3` (Edge's window background), span exactly the Edge window, end at
fixed 34-row strip boundaries (1080 / 32) and often start mid-row, with the page text still drawn
over them: a window caught in the middle of its update. Where they come from, step by step (VM
`tether-bench`, no GPU, `DesktopImageInSystemMemory = TRUE`):

| Test | Frames with a band |
| --- | --- |
| Encoded stream recorded in the viewer, decoded with ffmpeg (reference decoder) | 14–36 % |
| The same streams decoded with WebCodecs on the RTX 3080 (hardware, NV12) vs software | identical, 0 frames differ |
| Desktop Duplication alone in the VM (`scrap` example `tearprobe`, no encoder, no session) | 13–52 % |
| … with a staging copy instead of `MapDesktopSurface` | 16 % |
| … re-read 2–40 ms later while the frame is held | band unchanged (a stable, torn snapshot) |
| GDI `BitBlt` of the screen, same page, same time | 0 of ~1,500 |
| Hyper-V console framebuffer (what the VM displays) | 0 of 80 |
| Windows.Graphics.Capture | 0 of ~2,400 |

So the encoder, the network, the hardware decoder and the canvas are not involved: Desktop
Duplication itself delivers frames that were never on the screen. GDI is no way out (one `BitBlt`
of the screen takes ~50 ms on the VM, even for a 16-px column).

## Fix

`vendor/rustdesk/libs/scrap/src/dxgi/wgc.rs`: where the duplicated desktop image is in system
memory (GPU-less hosts: WARP / display-only adapters, rotation 0), the capturer reads frames with
**Windows.Graphics.Capture** (borderless, no cursor) instead of Desktop Duplication. GPU hosts are
unchanged. If the API, a borderless session or a frame fails, it falls back to Desktop Duplication;
`TETHER_CAPTURE_DXGI=1` forces it. The frame pool has one buffer, given back as soon as the
frame is copied: DWM then composes at most one capture copy per encoded frame. Two buffers made
DWM copy at every composition (+20–50 ms latency, more CPU); holding the buffer while encoding
halved the frame rate when only Edge was animating (a composition during the encode was lost).

## Result (VM, same workload, 2026-11-04)

Host TetherQS (0.5.4 base vs fix) in the VM, technician in the VM (minimized), VP9, Edge welcome
page animating, VM clock barcode (`tools/bench/probe.ps1`) topmost. Latency = barcode → picture
drawn in the viewer (same clock). 30 s each, interleaved; VM CPU from the Hyper-V host.

| Build | Latency mean / p95 | Remote frames/s | Longest draw gap | kbit/s | VM CPU |
| --- | --- | --- | --- | --- | --- |
| 0.5.4 (Desktop Duplication) | 62.5 / 85, 61.5 / 87 ms | 19.2, 20.9 | 155, 109 ms | 114, 129 | 23.2, 20.7 % |
| fix (Windows.Graphics.Capture) | 65.9 / 99, 63.3 / 93 ms | 21.2, 21.8 | 136, 78 ms | 119, 117 | 22.6, 22.5 % |

Same frame rate, bitrate and VM CPU; latency +3 ms on average (0.5.4 alone ranged 59–74 ms over
ten runs of this setup). TetherQS itself uses more CPU (~57 vs ~40 % of a core: the copy
runs in its process instead of in DWM's), the VM total is the same.

The barcode window (repainted continuously) suppresses the artifact, so the band counts come from
runs without it (welcome page reloaded in a new Edge window, technician minimized, 30 s, stream
recorded in the viewer and decoded with ffmpeg): 0.5.4 10.2 % of 618 and 0.6 % of 510 frames with
a band (the artifact comes and goes with Edge's state), fix 0 of 552 and 0 of 445. In-VM capture alone (`tearprobe`):
Desktop Duplication 13–52 %, Windows.Graphics.Capture 0 % in every run.
