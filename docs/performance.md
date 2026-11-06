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

# Stress test under heavy screen changes (2026-11-06)

Tools: `tools/bench/stress.html` on the host (the `probe.html` barcode plus a workload that cycles
by wall-clock minute: idle, scrolling text, a new full-screen "photo" every 2 s, full-screen noise
every frame), `tools/bench/stress-sampler.mjs` on the viewer (decodes the barcode from the session
canvas over CDP every 250 ms), `tools/bench/stress-report.py` (per phase; latency includes the
viewer-host clock offset, so compare each phase against idle), `stress-matrix.sh` (codec matrix),
`stress-exp.sh` (one experiment), `inv.mjs` (invoke a Tauri command in a viewer).

## Mac host (MacBook Pro 14", direct LAN connection), before the fixes

Codec Auto: the delay grew to **67 s median, 135 s p95**; the viewer quit after a few minutes.
Causes, both fixed in 0.5.15:

1. The Mac streams a Retina display at its logical size, here 1728x1117. The odd height made the
   viewer's WebCodecs decoders fail (`EncodingError: Decoding error` for VP9 and AV1; locally, at
   an even size, the same decoders work). Every Mac session fell back to native decoding.
2. The native fallback sent every decoded picture (about 8 MB at that size) over the Tauri IPC
   channel without waiting, so the queue grew without bound.

Fixes: the Mac host captures at even dimensions (`even()` in `libs/scrap/src/quartz/display.rs`);
the native path sends a picture only after the window drew the previous one (`TauriSink::video`,
`session_frame_drawn`).

## Mac host after the fixes (2026-11-06 morning, same Mac and stress page)

Extra delay over idle, median / p95 (ms). The viewer decoded with WebCodecs in every run (no
`webcodecs failed` line); the stream is 1728x1116.

| Build | Codec | scroll | photo | noise | stalls >= 0.5 s |
| --- | --- | --- | --- | --- | --- |
| host 0.5.13, viewer 0.5.15 (bounded native path) | Auto | +68 / +329 | -3 / +222 | +167 / +354 | 5 (max 0.8 s) |
| host 0.5.15, viewer 0.5.15 | Auto (H.265) | +4 / +18 | +158 / +5519 | +10 / +24 | 3 (~2.8 s) |
| | H.265 | +3 / +19 | +1 / +78 | +12 / +4898 | 4 (~2.7 s) |
| | VP9 | +5 / +18 | -4 / +12 | +182 / +331 | 0 |

Before the fixes the same test reached 67 s median and the viewer quit. Open: the rare ~2.7 s
freezes with H.265 (the hardware path Auto picks on a Mac) when a whole new picture arrives;
VP9 shows none on this Mac.

## Windows host (GPU-less 1080p Hyper-V VM, TetherQS, corporate proxy path)

Extra delay over idle, median / p95 (ms); stalls = spans >= 0.5 s without a new picture.

| Build | Codec | scroll | photo | noise | stalls |
| --- | --- | --- | --- | --- | --- |
| 0.5.15 before the encoder change | Auto (VP9) | +112 / +453 | 0 / +76 | +955 / +1748 | 37 (1.6 s) |
| | VP8 | +716 / +3064 | +80 / +1299 | +1481 / +3335 | 104, one reconnect (18.8 s) |
| | AV1 | +22 / +77 | -2 / +71 | +1708 / +2369 | 1 |
| 0.5.15 (quantizer headroom under overload) | Auto (VP9) | +232 / +484 | -2 / +72 | +927 / +1720 | 66 (1.6 s) |
| | VP8 | +309 / +626 | -3 / +53 | +1224 / +2293 | 71, **no reconnect** |
| | VP9 | +150 / +452 | -2 / +59 | +937 / +1727 | 70 (1.6 s) |
| | AV1 | **+15 / +56** | -2 / +61 | **+342 / +1439** | **0** |

- Opening a picture (photo phase) adds under 80 ms with every codec but VP8.
- The VP8 reconnect was the 3 s relay-silence check: one huge noise frame sat whole in the TLS
  proxy (it forwards WebSocket messages only when complete). With the quantizer allowed up to 63
  under overload the frames stay small enough.
- AV1 is the smoothest codec on this host in every phase, but Auto picked VP9: the one-time AV1
  speed test (`test_av1`) failed its 90 ms key-frame limit here and stored `N` for good. The limits
  are now 400 ms (key frame) / 50 ms (other frames) and the option is renamed (`av1-test-2`) so
  every host tests once more. Measured 2026-11-06 on the same VM: the test now passes (key frame
23 ms, other frames 8 ms) and Auto picks AV1. Auto before -> after, extra delay p95: scroll
+475 -> +121 ms, photo +62 -> +80 ms, noise +1679 -> +1324 ms (median +876 -> +349 ms), stalls
68 -> 0 in 8 min.

## Long session

Mac (lock screen, direct LAN, viewer 0.5.15): 46 min until the run was stopped for low memory on
the test PC: no errors, no reconnect, viewer 250-270 MB and WebView 141 MB, steady.

Windows VM (TetherQS 0.5.15, stress page cycling all phases, Auto = VP9, corporate proxy path):
2 h, 27,317 samples, every one decoded, no error, no reconnect. Viewer (~190 MB with WebView),
proxy (~180 MB) and TetherQS in the VM (380 MB) stayed flat. The picture got slower in the second
hour, though:

| | median | p95 | longest stall |
| --- | --- | --- | --- |
| hour 1 | 139 ms | 1579 ms | 1.6 s (noise) |
| hour 2 | 196 ms | 1778 ms | 5.8 s (noise), 4.2 s (photo) |

Open: the cause of the second-hour slowdown (no memory growth on either side; next step is
encoder time per frame and relay load over time). AV1, smoother on this host in every phase, would
likely change the picture; see [Unreleased] in the CHANGELOG.

Mac with real content (host and viewer 0.5.15, stress page cycling all phases, Auto): 16 min
until the run was stopped for low memory on the test PC (other applications): 486 samples, all
decoded, no stall, no error; every phase within +34 ms (p95) of idle; viewer 67 MB + WebView
153 MB, flat.

Second Mac run with real content (09:23-10:09, Auto, Safari windowed next to other apps): 46 min
until it was stopped for low memory on the test PC (other applications). The barcode was covered
for the first 24 min; the 22 min it was visible: 648 samples, no stall, no error, every phase
within +35 ms (p95) of idle; viewer 64-66 MB + WebView 142-144 MB, flat over the whole run.

Mac 1-hour long run (host and viewer 0.5.16, stress page full-screen in Safari, Auto, direct LAN,
12:17-13:14): 57 min, 3245 samples, no stall >= 0.5 s, no error, no reconnect. Every phase within
+87 ms (p95) of idle. Viewer 57-58 MB + WebView 139-148 MB, flat. 18 % of the samples had no
visible pattern because another agent brought Edge to the front on the Mac; they are excluded.

# Freezes in ~3 s steps when the content gets heavy, and the long-run check (2026-11-06)

## Mac host, H.265: what the freezes are

The morning runs (`mac-after-h265`, `mac-after-auto`, 4 min each) had one episode each, not
several independent freezes: at the switch to the noise phase (H.265) or 15 s into the photo
phase (Auto = H.265), 4 pictures arrived **3.02–3.03 s apart in host time** (the barcode values of
consecutive pictures), each shown ~2.9 s after the host drew it; then the stream was back at idle
latency for the rest of the phase (H.265 in the 45 s of noise after the episode: +9 / +25 ms median / p95 over idle; VP9 in noise on the same Mac: +183 / +331 ms).

- 3.0 s is the video loop's wait for the connection to take the previous frame
  (`video_service.rs`, `timeout_millis = 3_000`): the connection takes the next frame only after
  it has written the previous one into the stream, so a full link makes the loop wait, and at
  3 s it gives up and encodes the next frame. Nothing else on the host or the viewer runs on a
  3 s period with frames in flight (the viewer's key-frame retry is 1 s, its decode queue resync
  would show a burst, not one picture per 3 s).
- VideoToolbox is not latency-free (`HwRamEncoder::latency_free`): its packet for a frame comes
  out with the next encode call, so each picture is one loop step (~3 s) old when it is sent.
- It ended after ~11 s: the probe-based controller needs two to three bad TestDelay replies
  (the probes queue behind the same video) plus a 3 s cooldown per ratio step, so it took three
  steps to bring the bitrate under what the path carried.

So the trigger is a burst above what the path carries when the content switches from light to
full-screen (VideoToolbox's average-bitrate rate control lets a scene change overshoot), and the
multi-second part is the slow confirmation. Hardware H.265 itself is not the problem: in steady
noise it was the fastest codec on that Mac. Auto stays as it is.

## Fix: the host reacts to its own backlog

`VideoQoS::on_send_backlog` (`video_qos.rs`), called from the video loop's wait: once a frame has
not been taken for 1 s (`BACKLOG_MS`), the bitrate is halved at once (at most once a second, not
below the usual floor), and replies to probes sent before the cut neither cut again nor lower the
fps. Without ABR (encoders that cannot change the bitrate) the fps is halved instead, down to the
usual 5 fps floor. The normal controller earns the bitrate back as before.

Closed-loop simulation (`video_qos/tests/sim.rs`, `sim_switch_to_heavy_content_drains_fast_with_backlog_signal`):
office content, then full-screen video at 2.7x what a 1.5 Mbit/s link carries, 20 seeds. Longest
span with >= 1 s of queue after the switch, worst seed: CBR encoder (VP8/VP9/AV1 model) 59.5 s
without the signal (the queue never drained in the 60 s) -> 2.5 s with it; fixed-rate encoder
(hardware model) 30 s -> 5.5 s. The existing scenarios keep their bounds with the signal on and
improve where the path really shrinks (queue p95, p90 over 20 seeds): `bandwidth_halved_30`
2083 -> 592 ms, `bandwidth_halved_fixed_rate_30` 636 -> 524 ms,
`bandwidth_halved_fixed_rate_no_abr_30` 1688 -> 912 ms; Wi-Fi with stalls 299 -> 276 ms, clean
links unchanged.

Not covered by the model: the real VideoToolbox frame sizes after a scene change. Whether the
Mac episode is gone has to be measured on the Mac (plan below).

## Windows VM, 2 h: the second hour

Per 15 minutes (extra delay over idle, median / p95 ms, from `vmlong-auto.jsonl`):

| minutes | idle | scroll | photo | noise |
| --- | --- | --- | --- | --- |
| 0–15 | -3 / 40 | 182 / 454 | -3 / 61 | 941 / 1726 |
| 45–60 | -4 / 36 | 135 / 453 | -4 / 60 | 920 / 1711 |
| 90–105 | -2 / 38 | 187 / 466 | -3 / 61 | 934 / 1728 |
| 105–120 | 65 / 419 | 581 / 1912 | 253 / 4956 | 1288 / 4258 |

Flat for 105 minutes, then every phase got worse at once from about 06:04, the idle phase too
(nothing to encode there but the clock barcode). That is the environment (the VM, this PC or the
proxy path), not a controller state that builds up over an hour. The controller was checked
anyway: `sim_two_hours_of_stress_cycles_do_not_drift` runs two hours of the stress cycle (idle,
scroll, photo every 2 s, noise that a small software encoder cannot fit into a 3 Mbit/s path)
for CBR, fixed-rate and non-ABR encoders over 4 seeds and asserts that hour 2 ends every cycle
in the same state as hour 1 (ratio and fps after the idle minute, queue p95, longest backlog).
It passes: ratio 0.670 / 0.670 and 30 / 30 fps after every idle minute in both hours, queue p95
hour 1 / hour 2 e.g. 530 / 522 ms (CBR), 397 / 402 ms (fixed-rate), 752 / 734 ms (fixed-rate
without ABR). No drift found in `video_qos.rs`; nothing to fix there.

To tell next time what changes, the host logs one line per minute while video is sent
(`fluxrt video/min`, replaces the 5 s `fluxrt encode` line): codec, fps, encode time avg / p95 /
max, kbit/s, largest frame, key frames, encode calls without output, frames the connection did
not take within 1 s / 3 s and the longest wait, and the QoS state (fps, ratio, target kbit/s).
A backlog cut is logged as `video backlog: ...` (at most every 10 s).

## Start-up fast path

The tauri-only fast path in `user_network_delay` set the frame rate to the viewer's cap on any
clean reply. Two `video_qos` tests failed with it (they were also written for 30 fps; they now
derive their values from `FPS`): at a 120 fps cap with a fixed-rate encoder the start-up queue
reached 4.0 s on a 2.5 Mbit/s path (15 -> 120 fps at once), and after a restore that congested
the jump returned to the failed level at once. It now doubles per clean reply and only during
start-up (until a reply shows queue growth, a reduction or the cap). Simulation: start-up queue
peak at most 1.16 s for every cap (was 4.0 s at 120); clean relay reaches 60 fps in 1.1 s; the
jittery Wi-Fi scenario takes 8.1 s to 90 % of 60 fps (was 0.1 s) with the same mean target
(58.3 fps).

## How to check on real hosts

Mac (host with this build, viewer on the PC, direct LAN): `tools/bench/stress-exp.sh <label> h265 0 4`
and `... auto 0 4`, three runs each, stress page full-screen in Safari; count stalls >= 0.5 s with
`stress-report.py` and look in the host log for `video backlog:` (each cut) and the per-minute
`fluxrt video/min` lines (`wait >=1s`, `largest frame`, `kbit/s` against `target`). Expected: no
run of pictures 3 s apart; at most one ~1 s hitch where the content switches.
Windows VM: the 2 h `vm-long` run with the host log kept; the `fluxrt video/min` lines show
whether a slow stretch comes with longer encode times (CPU), backlog waits (path) or a QoS state.

# Sharpening in the viewer (2026-11-06)

Text and fine lines of the remote picture get soft when the viewer scales it to the window (the
usual case: a 2560×1440 screen in a 1600×900 window) and from lossy coding. The viewer now scales
and sharpens in one WebGL2 pass (`apps/desktop/src/lib/sharpen.ts`); the earlier 2D canvas path
stays for **Off**, for webviews without WebGL2 and after a lost GPU context.

## Design

- **One pass.** The decoded `VideoFrame` is uploaded with `texImage2D` (GPU to GPU, no CPU copy) and
  drawn with a fragment shader into a canvas sized to its on-screen device pixels
  (`ResizeObserver` `device-pixel-content-box`; rounded CSS size × DPR where that box is missing,
  WebKit). Scaling is bilinear. Natively decoded pictures (`PKT_RGBA`) are uploaded as bytes and
  BGRA is swizzled in the shader instead of a JS loop.
- **Filter: clamped luma unsharp mask with a noise gate.** 5 taps (centre + cross; one output pixel
  apart when shrinking, one source pixel otherwise). The luma Laplacian is added to all three
  channels (no colour fringes), scaled by `smoothstep(0.02, 0.08, local luma range)` so flat areas
  and compression noise below ~5–20 levels are left alone, and the result is clamped per channel to
  the min/max of the taps: it cannot overshoot its neighbourhood, so no halos or ringing. A CAS
  port was not used: CAS backs off most on high-contrast edges, which is exactly black-on-white
  text, and sharpens low-contrast areas most, which is where block noise lives.
- **Strength.** Auto: k = 0.2 when the picture is scaled (either way), 0 at 1:1 (a box that differs
  by a rounding pixel counts as 1:1). Strong: k = 0.5 at every size. Off: the 2D canvas.
- **Switching** needs a new canvas element (a canvas keeps its first context): the canvas is keyed
  by the path, the pipeline takes over the new element and carries the current picture over
  (`readPixels` of the source texture, or the old 2D canvas as a texture), so a switch neither
  blanks the picture nor asks the host for a key frame. A lost context falls back to the 2D canvas
  for the rest of the window and asks for a key frame.
- `preserveDrawingBuffer: true` keeps the canvas readable after compositing (stress sampler, mode
  switch); its cost is inside the measurements below.

## Picture quality: `apps/desktop/e2e/sharpen-quality.mjs`

Deterministic 1920×1080 fixture (`e2e/sharpen-fixture.html`): text at 9–16 px (black, grey, white
on blue, red), 1 px lines with 1–4 px gaps, a star, a photo-like area, a flat grey area with ±6
levels of noise. Encoded and decoded with WebCodecs (12 frames of a static screen, or the key frame
alone), drawn at 1:1, 0.75× and 1.5× by the 2D path (Off) and the shader, read back with
`drawImage` of the canvas (as the stress sampler does) and compared with the original scaled by
the browser's high-quality resampler (the ideal picture). Metrics on luma: **sharpness** = mean
|Laplacian| relative to the ideal (1.0 = as sharp as the ideal, below = soft); **MAE** against the
ideal (levels of 255); **overshoot** = mean amount the picture leaves the ideal's 3×3 min/max
(halos, ringing). Headless Edge, ANGLE D3D11 on the RTX 3080. Full tables:
`tools/bench/results/sharpen/quality.txt` and `quality-*.json`.

Text area, sharpness / MAE / overshoot (Auto = k 0.2 when scaled, Strong = k 0.5):

| Stream | Scale | Off | Auto | Strong |
|---|---|---|---|---|
| VP9 1 Mbit/s, static (12 frames) | 1:1 | 1.05 / 2.26 / 0.93 | = Off | 1.20 / 2.67 / 1.10 |
| | 0.75× | 1.01 / 1.83 / 0.63 | 1.10 / 1.95 / 0.70 | 1.18 / 2.46 / 0.75 |
| | 1.5× | 1.00 / 2.07 / 0.73 | 1.10 / 2.00 / 0.76 | 1.23 / 2.41 / 0.79 |
| VP9 1 Mbit/s, key frame only | 1:1 | 0.93 / 5.04 / 1.47 | = Off | 1.09 / 4.83 / 1.62 |
| | 0.75× | 0.94 / 3.91 / 1.02 | 1.02 / 3.79 / 1.09 | 1.10 / 3.94 / 1.16 |
| | 1.5× | 0.90 / 4.21 / 1.23 | 1.00 / 4.03 / 1.26 | 1.12 / 4.08 / 1.29 |
| VP9 3 Mbit/s, static | 1:1 | 1.03 / 1.10 / 0.43 | = Off | 1.16 / 1.83 / 0.55 |
| | 0.75× | 1.01 / 0.91 / 0.28 | 1.09 / 1.22 / 0.34 | 1.17 / 1.86 / 0.38 |
| | 1.5× | 0.98 / 1.21 / 0.31 | 1.08 / 1.21 / 0.33 | 1.20 / 1.76 / 0.35 |
| H.264 1 Mbit/s, static | 1:1 | 0.95 / 3.04 / 0.83 | = Off | 1.10 / 2.92 / 0.95 |
| | 0.75× | 0.96 / 2.30 / 0.58 | 1.04 / 2.24 / 0.65 | 1.12 / 2.53 / 0.69 |
| | 1.5× | 0.92 / 2.60 / 0.67 | 1.02 / 2.40 / 0.69 | 1.14 / 2.60 / 0.70 |

- Soft pictures (key frames, H.264 at 1 Mbit/s, enlarged ones) are 4–10 % less sharp than the
  ideal; Auto brings them to 1.00–1.04 and lowers their error (H.264 1.5×: MAE 2.60 → 2.40).
- Well-coded static pictures are already as sharp as the ideal; Auto makes them ~10 % sharper at a
  small MAE cost (+0.1 to +0.3 levels). At 1:1, k 0.2 came out neutral on average (MAE −0.23 …
  +0.22 over the four streams), so Auto skips 1:1.
- Overshoot grows by 2–12 % with Auto (the clamp keeps it inside the decoded neighbourhood), by
  4–28 % with Strong. Flat noisy area: sharpness unchanged (0.026 → 0.026 at 1:1, 0.042 → 0.044 at
  1.5×): the noise gate does not amplify compression noise or blocking.
- k 0.8 and 1.2 (swept) raise MAE and overshoot on every stream; not offered.

Comparison strips (original | Off | Auto | Strong, zoomed nearest-neighbour, VP9 1 Mbit/s static):
`tools/bench/results/sharpen/s{1,0.75,1.5}-{text,lines,photo}.png`.

Draw cost in the same browser, each draw waited for with a 1-pixel readback (200 draws, ms):
2D `drawImage` 0.57–1.39, WebGL without sharpening 0.29–0.62, WebGL with k 0.2 0.29–0.43. The
sharpening taps cost nothing measurable next to the upload.

## In a session: `apps/desktop/e2e/sharpen-e2e.mjs`

Two debug instances (host `sharp-host`, viewer `sharp-view`) on a hidden desktop of this PC
(RTX 3080, 2560×1440 at 125 %), direct LAN, H.265 (hardware encode and decode). The host streams
the screen with `tools/bench/stress.html?phase=1` (scrolling text + barcode) on top; the viewer
window shows it at 1600×900 device pixels, so Auto sharpens (2560×1440 → 1600×900, k 0.2). Phases
of 40 s, Off → Auto → Off → Auto → Strong; latency from `tools/bench/stress-sampler.mjs` (barcode
every 100 ms, ~370 samples per phase; host and viewer share the clock, the absolute value includes
the sampler's own read and the 60 Hz capture). Result: `tools/bench/results/sharpen/e2e.json`.

| | Off (2D) | Auto | Off (2D) | Auto | Strong |
|---|---|---|---|---|---|
| pictures shown / s (= received) | 50.3 | 51.1 | 51.5 | 50.9 | 50.1 |
| draw, CPU (ms per picture) | 0.17 | 0.11 | 0.16 | 0.12 | 0.12 |
| latency p50 / p95 (ms) | 137 / 154 | 135 / 152 | 132 / 151 | 136 / 155 | 138 / 156 |

Debug measurement mode (`localStorage["tether.renderSync"] = "1"`, read once a second: each draw
waits for the GPU with a 1-pixel readback, so the time includes the GPU work; for measuring only):

| | Off (2D) | Auto | Strong | Off (2D) | Auto |
|---|---|---|---|---|---|
| draw incl. GPU (ms per picture) | 4.54 | 3.16 | 3.22 | 4.90 | 3.27 |

- No extra time per picture: the WebGL path is cheaper than the 2D path (−0.05 ms CPU, −1.5 ms
  with the GPU wait). Target was < 0.5 ms extra at 1080p.
- No frame-rate drop (every phase shows every received picture, 50–51.5 / s) and no latency change
  beyond the run-to-run spread (this run: Off 132–137 ms p50, Auto 135–136, Strong 138; an earlier
  run of the same script: Off 115–119, Auto 119–120, Strong 117).
- Same run: Original size → canvas 2560×1440 = its device pixels, Auto skips 1:1 (`auto 0.00`);
  a 1100×650 viewport (DevTools emulation) → canvas 1375×773 = its device pixels at 125 %;
  30 switches Off/Auto/Strong → WebGL again, picture moving, no "too many WebGL contexts";
  `WEBGL_lose_context` → the 2D canvas takes over, picture back after a key frame (317 ms);
  the stress sampler decodes the barcode from both canvases; JS heap 63.7 MB before and 57.8 MB
  after the switches. One texture per pipeline, re-specified per picture (no per-frame allocations
  that could leak).

## Not verified

- macOS WebKit (WKWebView): WebGL2 and `texImage2D(VideoFrame)` exist in Safari 17+, but this was
  not run on a Mac. Without `device-pixel-content-box` the canvas size is CSS size × DPR rounded,
  which can be a pixel off (slight resampling by the compositor). Without WebGL2 the viewer uses
  the 2D canvas as before.
- GPU-less viewers (WARP/SwiftShader): not measured; the shader reads 5 texels per pixel.
- Sessions longer than the ~12 min of the e2e run; GPU memory was not read directly.

### "Regression" in the VM on 2026-11-06 evening: the test proxy, not Tether

An 8-minute Auto run against TetherQS 0.5.17 in the Hyper-V VM showed full-screen noise at
+4120 / +9838 ms (median / p95 over idle) and 17 stalls, against +349 / +1324 ms and none for
0.5.16 that afternoon. The A/B runs right after pointed away from Tether:

| Run (same VM, same viewer) | noise median / p95 over idle | stalls |
|---|---|---|
| 0.5.17 host, sharpening Auto | +4120 / +9838 ms | 17 |
| A: 0.5.16 host, sharpening Auto | +18402 / +42572 ms | 51 |
| B: 0.5.16 host, sharpening Off | +39275 / +65472 ms | 169 |

The older host was worse, sharpening off was worse, and every run was worse than the one before.
The VM reaches the relays only through the TLS-inspecting test proxy (`tools/bench/corp_proxy.py`,
mitmdump), which had been running since 04:15: it kept every WebSocket message of a connection
in memory and appended one JSON line per video frame (116 MB log). It now logs the first 20
frames of a connection, counts the rest and keeps none. A clean rerun was cut short (Claude
Code's low-memory reaper), so the after-number is still open; the VM end-to-end test against
0.5.18 through the fixed proxy passed 5/5.
