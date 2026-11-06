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

## How to check on real hosts

Mac (host with this build, viewer on the PC, direct LAN): `tools/bench/stress-exp.sh <label> h265 0 4`
and `... auto 0 4`, three runs each, stress page full-screen in Safari; count stalls >= 0.5 s with
`stress-report.py` and look in the host log for `video backlog:` (each cut) and the per-minute
`fluxrt video/min` lines (`wait >=1s`, `largest frame`, `kbit/s` against `target`). Expected: no
run of pictures 3 s apart; at most one ~1 s hitch where the content switches.
Windows VM: the 2 h `vm-long` run with the host log kept; the `fluxrt video/min` lines show
whether a slow stretch comes with longer encode times (CPU), backlog waits (path) or a QoS state.
