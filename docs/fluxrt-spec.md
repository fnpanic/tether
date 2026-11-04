# FluxRT — behavioural spec

FluxRT is Tether's screen-streaming profile. It is a **clean-room** design: it is written only from
publicly available descriptions of how fast remote-desktop codecs behave and from black-box
measurements of other products running as a normal user would run them. No vendor binary was
decompiled, disassembled or debugged to produce it, and nothing here is wire-compatible with any
other product.

## What "fast remote desktop" means (public characteristics)

Remote-desktop vendors, including AnyDesk for its DeskRT codec, publicly describe these goals:

| Goal | Why it matters |
| --- | --- |
| Codec tuned for desktop content (text, flat UI, scrolling) rather than camera video | Sharp text at low bitrate |
| Only changed screen regions cost bandwidth | Idle desktop ≈ 0 kbit/s |
| Very low added latency, frames never queue | Mouse feels local |
| Frame rate up to the display refresh (60 fps) on good links | Smooth scrolling, video |
| Usable on slow links (≈100 kB/s) | Degrades frame rate before legibility |
| Fast recovery to full sharpness after motion stops | Text readable immediately |

## How FluxRT gets there (on the RustDesk core)

| Property | Mechanism | Where |
| --- | --- | --- |
| Hardware encode | Prefer H.264 → HEVC → AV1 hardware encoders (NVENC/AMF/QSV/VideoToolbox), VP9 software fallback | `scrap` + `hwcodec` |
| No B-frames, no lookahead, 1-frame VBV | Low-latency encoder presets (hwcodec defaults) | host encoder config |
| Zero-copy-ish decode on the viewer | Encoded frames go untouched to the webview, decoded by **WebCodecs** (`optimizeForLatency`) in hardware, drawn on a desynchronised canvas | `tauri_ui::video_passthrough`, `src/lib/video.ts` |
| Never show stale frames | Viewer drops to the newest frame when the decode queue backs up and re-requests a key frame until one arrives (`KeyRecovery`, limit 24 queued frames) | `lib/video.ts` |
| Local cursor | Remote cursor shape becomes the local CSS cursor, so pointer motion has zero network latency | `Session.tsx` |
| Input batching | Pointer moves coalesced to one per animation frame; clicks/keys sent immediately | `Session.tsx` |
| Idle = no traffic | Unchanged frames are not encoded (RustDesk frame comparison) | `scrap` capturer |
| Adaptive rate | RustDesk `video_qos` lowers FPS first, then bitrate, based on RTT and decode feedback | `server/video_qos.rs` |
| Sharp text mode | "Sharp text" quality = highest bitrate ratio; 4:4:4 chroma where both sides support it | `session_quality` |
| Relay always | Sessions go through the relay (`hbbr`) over WebSocket/443 since 0.2; no UDP hole punching | rendezvous |

## Targets (pass criteria)

Measured with the in-session statistics overlay on a 1920×1080 desktop, both ends on the same LAN
unless stated.

| Scenario | Target |
| --- | --- |
| Idle desktop | < 50 kbit/s |
| Typing in an editor | ≥ 30 fps, < 1 Mbit/s |
| Scrolling a long web page | ≥ 55 fps, < 8 Mbit/s |
| Viewer decode time (WebCodecs, hardware) | < 4 ms per frame |
| Added latency (input → visible change), LAN | < 30 ms |
| 200 ms RTT, 2 % loss (clumsy) | usable: no queue build-up, readable text within 1 s of motion stopping |
| Relay vs P2P | same fps, latency increase ≈ relay RTT only |

## Black-box comparison protocol

To compare against any other product, run both products on the same pair of machines and the same
network. Use the product as a normal user would, and do not inspect its binaries.

1. Static desktop for 30 s. Measure throughput with Windows Resource Monitor or `pktmon` byte
   counts. Packet *sizes and timing* only, because the payload is encrypted.
2. Type a fixed paragraph in Notepad. Record fps and throughput.
3. Scroll a long page at a constant rate for 10 s.
4. Play a 1080p video full screen.
5. Repeat steps 2–4 under `clumsy` with 200 ms lag and 2 % drop.
6. Latency: show a millisecond clock on the host. Take one photo containing both screens with a
   phone camera. The difference between the two clocks is glass-to-glass latency.

Results go in `docs/benchmarks.md`. This comparison needs **two physical machines**. A single
machine cannot run a session of another product against itself.
