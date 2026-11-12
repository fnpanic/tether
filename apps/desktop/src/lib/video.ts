// Hardware video decoding in the webview (WebCodecs) for frames forwarded raw from the Rust core.
// Drawing: WebGL2 with scaling + sharpening (sharpen.ts), or the 2D canvas (sharpening off, no WebGL).
// Packet layout (little endian, see vendor/rustdesk/src/tauri_ui/mod.rs):
//   kind u8 | codec u8 | flags u8 (bit0 key, bit1 bgra) | display u8 | w u32 | h u32 | pts u32 | payload

import { GlRenderer, type SharpenMode } from "./sharpen";

export const PKT_ENCODED = 0;
export const PKT_RGBA = 1;
/** Exact rows of a still picture. `codec` 0: `width`, `height` are the strip's, `pts` its first
 * row, the data its hash (u32) and RGBA rows. `codec` 1: the strips held here that go back over
 * the frame before (u32 pairs of first row and hash). */
export const PKT_EXACT = 2;
export const CODEC = { vp8: 1, vp9: 2, av1: 3, h264: 4, h265: 5 } as const;
export type CodecName = keyof typeof CODEC;

const CODEC_STRING: Record<number, string> = {
  1: "vp8",
  2: "vp09.00.10.08",
  3: "av01.0.08M.08",
  4: "avc1.64002A",
  5: "hvc1.1.6.L153.B0",
};

// The host converts VP8/VP9/AV1 input with libyuv (BT.601, limited range) and leaves the bitstream's colour
// description unset; WebCodecs then decodes with BT.709, which shifts saturated colours by up to 30 levels
// (pure green 192 -> 163, blue gains 16 green). Name the host's matrix; primaries/transfer are the sRGB the
// screen had, so nothing else is converted. Hardware H.264/H.265 hosts convert on the GPU: left as decoded.
const HOST_BT601: VideoColorSpaceInit = { primaries: "bt709", transfer: "iec61966-2-1", matrix: "smpte170m", fullRange: false };

export function decoderConfig(codec: number): VideoDecoderConfig {
  const libyuv = codec === CODEC.vp8 || codec === CODEC.vp9 || codec === CODEC.av1;
  return { codec: CODEC_STRING[codec], optimizeForLatency: true, hardwareAcceleration: "no-preference", ...(libyuv && { colorSpace: HOST_BT601 }) };
}

export interface Packet {
  kind: number;
  codec: number;
  key: boolean;
  bgra: boolean;
  display: number;
  width: number;
  height: number;
  pts: number;
  data: Uint8Array;
}

export function parsePacket(buf: ArrayBuffer): Packet {
  const v = new DataView(buf);
  const flags = v.getUint8(2);
  return {
    kind: v.getUint8(0),
    codec: v.getUint8(1),
    key: (flags & 1) !== 0,
    bgra: (flags & 2) !== 0,
    display: v.getUint8(3),
    width: v.getUint32(4, true),
    height: v.getUint32(8, true),
    pts: v.getUint32(12, true),
    data: new Uint8Array(buf, 16),
  };
}

/** Which codecs this webview can decode, probed once with low-latency configs. */
export async function probeCodecs(): Promise<CodecName[]> {
  if (typeof VideoDecoder === "undefined") return [];
  const out: CodecName[] = [];
  for (const [name, id] of Object.entries(CODEC) as [CodecName, number][]) {
    try {
      const r = await VideoDecoder.isConfigSupported({ codec: CODEC_STRING[id], optimizeForLatency: true });
      if (r.supported) out.push(name);
    } catch {
      /* unsupported */
    }
  }
  return out;
}

/** Ask again this often while no key frame has arrived (a request or its answer can be lost). */
export const KEY_RETRY_MS = 1000;
/** Drop deltas and resync only when the decoder is this many frames behind. Bursts of a few
 * frames are normal (relays and proxies coalesce packets) and decode in a few milliseconds. */
export const MAX_DECODE_QUEUE = 24;

/**
 * Key-frame recovery: once the pipeline needs a key frame it requests one right away and again every
 * KEY_RETRY_MS until a key frame arrives, so a lost request never freezes the picture.
 */
export class KeyRecovery {
  waiting = false;
  requests = 0;
  private since = 0;
  private last = -Infinity;

  /** Start (or keep) waiting; returns true when a request should be sent now. */
  need(now: number): boolean {
    if (!this.waiting) {
      this.waiting = true;
      this.since = now;
      this.last = -Infinity;
    }
    return this.poll(now);
  }

  /** While waiting: true when the previous request is overdue and must be repeated. */
  poll(now: number): boolean {
    if (!this.waiting || now - this.last < KEY_RETRY_MS) return false;
    this.last = now;
    this.requests++;
    return true;
  }

  /** Wait for a key frame that is already on its way (a new stream starts with one): no request now,
   * poll() asks only if it has not arrived after KEY_RETRY_MS. */
  expect(now: number) {
    if (!this.waiting) {
      this.waiting = true;
      this.since = now;
    }
    this.last = now;
  }

  /** A key frame arrived: returns how long the picture waited for it (ms), or 0 if it did not. */
  gotKey(now: number): number {
    if (!this.waiting) return 0;
    this.waiting = false;
    return now - this.since;
  }
}

/** Watchdog (Session): pictures go into the decoder but none come out for STALL_MS (at least
 * STALL_FRAMES of them), or none come from native decoding NATIVE_MS after switching to it. */
export const STALL_MS = 3000;
export const STALL_FRAMES = 10;
export const NATIVE_MS = 5000;

/** stage 0: WebCodecs watched; 1: switched to native decoding, a picture is due (`since`);
 * 2: another codec was requested, nothing more is done. */
export interface Watch {
  stage: 0 | 1 | 2;
  fed: number;
  drawn: number;
  stalled: number;
  since: number | null;
}
export const WATCH0: Watch = { stage: 0, fed: 0, drawn: 0, stalled: 0, since: null };

/**
 * One watchdog check (about once a second). `fed`/`drawn`: pictures given to the decoder / drawn
 * since the session started. It compares frames, not the time since the last one: a still screen
 * sends no frames and never triggers. Frames dropped while waiting for a key frame (first frame,
 * reconnect, display switch) are not fed. `active`: connected, not reconnecting, not view-only.
 */
export function watchdog(w: Watch, fed: number, drawn: number, now: number, active: boolean): [Watch, "native" | "codec" | null] {
  const s: Watch = { ...w, fed, drawn };
  if (drawn > w.drawn) return [{ ...s, stage: w.stage === 2 ? 2 : 0, stalled: 0, since: null }, null];
  if (!active) return [{ ...s, stalled: 0, since: w.stage === 1 ? now : null }, null];
  if (w.stage === 1 && w.since !== null && now - w.since >= NATIVE_MS) return [{ ...s, stage: 2, since: null }, "codec"];
  if (w.stage !== 0) return [s, null];
  s.stalled += fed - w.fed;
  if (s.stalled === 0) return [s, null];
  s.since ??= now;
  if (s.stalled >= STALL_FRAMES && now - s.since >= STALL_MS) return [{ ...s, stage: 1, stalled: 0, since: now }, "native"];
  return [s, null];
}

/** The decoder failed some other way and native decoding took over: a picture is due from it. */
export const watchNative = (w: Watch, now: number): Watch => (w.stage === 2 ? w : { ...w, stage: 1, stalled: 0, since: now });

export interface VideoStats {
  fps: number;
  /** Encoded packets received per second (before decoding). */
  inFps: number;
  coded: string;
  decodeMs: number;
  width: number;
  height: number;
  codec: string;
  /** Longest time without a new picture during the last second (ms). */
  maxGapMs: number;
  /** Key frames requested since the session started. */
  keyRequests: number;
  /** Delta frames dropped since the session started (waiting for a key frame). */
  dropped: number;
  /** Average time to draw a picture (ms, CPU; GPU included when `tether.renderSync` is set). */
  renderMs: number;
  /** Sharpening: "off", or the mode and the strength of the last draw. */
  sharpen: string;
}

/** Debug measurement mode (localStorage `tether.renderSync` = 1, read once a second): every draw
 * waits for the GPU, so renderMs includes GPU time. Adds latency; never on by default. */
function syncMode() {
  try {
    return localStorage.getItem("tether.renderSync") === "1";
  } catch {
    return false;
  }
}

interface Callbacks {
  /** Decoder broke or rejected the stream: switch this codec to native decoding. */
  onFail: (codec: number, display: number) => void;
  /** Need a key frame (after reset or a dropped delta). */
  onNeedKey: (display: number) => void;
  onSize: (w: number, h: number) => void;
  /** A natively decoded picture is on screen: the backend sends the next one only after this. */
  onRgbaDrawn?: () => void;
  /** The WebGL context was lost: attach a fresh canvas without WebGL. */
  onGlLost?: () => void;
}

export class VideoPipeline {
  private ctx: CanvasRenderingContext2D | null = null;
  private gl: GlRenderer | null = null;
  private sharpen: SharpenMode = "auto";
  private renderSum = 0;
  private sync = syncMode();
  private decoder: VideoDecoder | null = null;
  private codec = 0;
  private key = new KeyRecovery();
  private retryTimer: ReturnType<typeof setInterval> | undefined;
  private lastDraw = 0;
  private maxGap = 0;
  private dropped = 0;
  private pending = new Map<number, number>();
  private frames = 0;
  private decodeSum = 0;
  private lastStats = performance.now();
  private w = 0;
  private expected: { w: number; h: number } | null = null;
  private h = 0;
  stats: VideoStats = { fps: 0, inFps: 0, coded: "", decodeMs: 0, width: 0, height: 0, codec: "", maxGapMs: 0, keyRequests: 0, dropped: 0, renderMs: 0, sharpen: "off" };
  private packets = 0;
  private decodedSinceConfigure = 0;
  display = 0;
  /** Pictures given to the decoder (or received decoded) and pictures drawn, for the watchdog. */
  fed = 0;
  drawn = 0;
  /** Debug builds (TETHER_DEV_VIDEO_STALL), like a silently failing decoder: "webcodecs": the first
   * WebCodecs codec draws nothing; "native": no codec but VP8/VP9 draws, native decoding neither. */
  devStall = "";
  private devStallCodec = 0;
  private devDrop() {
    if (this.devStall === "native") return this.codec !== CODEC.vp8 && this.codec !== CODEC.vp9;
    return this.devStall === "webcodecs" && (this.devStallCodec ||= this.codec) === this.codec;
  }

  private canvas!: HTMLCanvasElement;

  constructor(canvas: HTMLCanvasElement, private cb: Callbacks, sharpen: SharpenMode = "off", gl = sharpen !== "off") {
    this.sharpen = sharpen;
    this.attach(canvas, gl);
  }

  /**
   * Draw into `canvas`: with WebGL (scaling + sharpening) when `gl`, else with the 2D canvas. A canvas
   * keeps the context it got first, so switching needs a new canvas element; the current picture is
   * carried over. Returns whether WebGL is in use (false: unavailable, the 2D canvas draws).
   */
  attach(canvas: HTMLCanvasElement, gl: boolean): boolean {
    if (canvas === this.canvas) return !!this.gl;
    const snap = this.gl ? this.gl.snapshot() : this.ctx && this.w ? this.canvas : null;
    this.gl?.dispose(true);
    this.gl = null;
    this.ctx = null;
    this.canvas = canvas;
    this.w = this.h = 0;
    if (gl) {
      this.gl = GlRenderer.create(canvas, () => this.cb.onGlLost?.());
      if (this.gl) this.gl.mode = this.sharpen;
    }
    if (!this.gl) {
      // desynchronized: bypass the compositor queue where supported → lower glass-to-glass latency.
      const ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
      if (!ctx) throw new Error("2d context unavailable");
      this.ctx = ctx;
    }
    if (snap) {
      this.resize(snap.width, snap.height);
      if (this.gl) this.gl.frame(snap, snap.width, snap.height);
      else if (snap instanceof ImageData) this.ctx!.putImageData(snap, 0, 0);
      else this.ctx!.drawImage(snap, 0, 0);
    } else if (this.decodedSinceConfigure > 0) this.needKey("renderer changed");
    return !!this.gl;
  }

  /** The last codec WebCodecs decoded (0: none). */
  get lastCodec() {
    return this.codec;
  }

  /** Drawing with WebGL (sharpening) rather than the 2D canvas. */
  get webgl() {
    return !!this.gl;
  }

  /** Sharpening mode for the WebGL path (Off is drawn by the 2D canvas, see attach). */
  setSharpen(m: SharpenMode) {
    this.sharpen = m;
    if (this.gl) {
      this.gl.mode = m;
      this.gl.draw();
    }
  }

  /** Need a key frame: ask now and keep asking until one arrives (see KeyRecovery). */
  private needKey(why: string) {
    if (!this.key.waiting) console.warn(`[video] waiting for a key frame: ${why}`);
    if (this.key.need(performance.now())) this.cb.onNeedKey(this.display);
    this.startKeyRetry();
  }

  private startKeyRetry() {
    this.retryTimer ??= setInterval(() => {
      if (this.key.poll(performance.now())) {
        console.warn(`[video] still no key frame, asking again (${this.key.requests})`);
        this.cb.onNeedKey(this.display);
      }
    }, KEY_RETRY_MS / 2);
  }

  private keyArrived() {
    const waited = this.key.gotKey(performance.now());
    if (waited > 300) console.warn(`[video] key frame after ${Math.round(waited)} ms`);
    clearInterval(this.retryTimer);
    this.retryTimer = undefined;
  }

  push(buf: ArrayBuffer) {
    const p = parsePacket(buf);
    if (p.display !== this.display) return;
    this.packets++;
    if (p.kind === PKT_EXACT) return this.exact(p);
    // A new picture: strips still waiting belonged to the one before it.
    this.strips.length = 0;
    if (p.kind === PKT_RGBA) return this.drawRgba(p);
    this.decode(p);
  }

  /** Exact rows of the parts of the picture that stand still (the host's exact_still.rs). They
   * follow a video frame, which may still be in the decoder: then they wait until it is drawn
   * (`output`). Every video frame replaces the whole picture; the host then names the strips
   * that are unchanged and they go back over it from `held`. Only with WebCodecs decoding: a
   * natively decoded picture takes another path and could arrive after its strips. */
  private strips: Packet[] = [];
  private native = false; // the picture on screen was decoded natively (PKT_RGBA)
  private held = new Map<number, { hash: number; rows: Uint8Array; h: number }>();
  private heldSize = { w: 0, h: 0 };
  private exact(p: Packet) {
    if (p.codec === 0) {
      // New rows are remembered at once, drawn or not: the host counts them as held from now on.
      if (p.width !== this.w || p.pts + p.height > this.h || p.data.length !== 4 + p.width * p.height * 4) return;
      const hash = new DataView(p.data.buffer, p.data.byteOffset, 4).getUint32(0, true);
      this.held.set(p.pts, { hash, rows: p.data.subarray(4), h: p.height });
    }
    if (this.native) return;
    if (this.pending.size) this.strips.push(p);
    else this.drawExact([p]);
  }

  private drawExact(ps: Packet[]) {
    let drawn = false;
    const rows = (y: number, data: Uint8Array, h: number) => {
      if (y + h > this.h || data.length !== this.w * h * 4) return;
      if (this.gl) this.gl.strip(data, y, this.w, h);
      else this.ctx!.putImageData(new ImageData(new Uint8ClampedArray(data.buffer as ArrayBuffer, data.byteOffset, data.length), this.w, h), 0, y);
      drawn = true;
    };
    for (const p of ps) {
      if (p.width !== this.w) continue;
      const v = new DataView(p.data.buffer, p.data.byteOffset, p.data.byteLength);
      if (p.codec === 1) {
        for (let i = 0; i + 8 <= v.byteLength; i += 8) {
          const s = this.held.get(v.getUint32(i, true));
          if (s && s.hash === v.getUint32(i + 4, true)) rows(v.getUint32(i, true), s.rows, s.h);
        }
      } else rows(p.pts, p.data.subarray(4), p.height);
    }
    if (drawn) this.gl?.draw();
  }

  private resize(w: number, h: number) {
    if (w === this.w && h === this.h) return;
    this.w = w;
    this.h = h;
    // Exact rows of another size are of no use; the same size again (a new canvas) keeps them.
    if (w !== this.heldSize.w || h !== this.heldSize.h) {
      this.held.clear();
      this.heldSize = { w, h };
    }
    // The WebGL canvas is sized to its on-screen pixels instead (GlRenderer), the shader scales.
    if (!this.gl) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    this.cb.onSize(w, h);
  }

  /** Time a draw; in the debug measurement mode wait for the GPU as well. */
  private timed(draw: () => void) {
    const t = performance.now();
    draw();
    if (this.sync) this.gl ? this.gl.sync() : this.ctx!.getImageData(0, 0, 1, 1);
    this.renderSum += performance.now() - t;
  }

  private configure(codec: number) {
    this.decoder?.state !== "closed" && this.decoder?.close();
    this.codec = codec;
    this.pending.clear();
    const dec = new VideoDecoder({
      output: (frame) => this.output(frame),
      error: (e) => {
        console.warn("[video] decoder error", e);
        this.decoder = null;
        this.cb.onFail(codec, this.display);
      },
    });
    dec.configure(decoderConfig(codec));
    this.decoder = dec;
    this.decodedSinceConfigure = 0;
    this.stats.codec = Object.entries(CODEC).find(([, v]) => v === codec)?.[0]?.toUpperCase() ?? "";
  }

  private decode(p: Packet) {
    this.native = false;
    if (!this.decoder || this.decoder.state === "closed" || p.codec !== this.codec) {
      this.configure(p.codec);
    }
    const dec = this.decoder!;
    // A fresh decoder (new codec, display switch, error) can only start from a key frame.
    if (this.decodedSinceConfigure === 0 && !p.key) this.needKey("decoder (re)started");
    if (this.key.waiting) {
      if (!p.key) {
        this.dropped++;
        return;
      }
      this.keyArrived();
    } else if (dec.decodeQueueSize > MAX_DECODE_QUEUE) {
      // Hopelessly behind: showing every queued picture would only add latency, so resync.
      this.dropped++;
      this.needKey(`decoder ${dec.decodeQueueSize} frames behind`);
      return;
    }
    this.decodedSinceConfigure++;
    this.fed++;
    const ts = p.pts * 1000;
    this.pending.set(ts, performance.now());
    try {
      dec.decode(new EncodedVideoChunk({ type: p.key ? "key" : "delta", timestamp: ts, data: p.data }));
    } catch (e) {
      this.pending.delete(ts); // it will never come out: nothing may wait for it
      console.warn("[video] decode threw", e);
      this.needKey("decode threw");
    }
  }

  private output(frame: VideoFrame) {
    if (this.devStall && this.devDrop()) return frame.close();
    this.stats.coded = `${frame.codedWidth}×${frame.codedHeight}`;
    // Use the visible picture size, not displayWidth: some HEVC streams carry aspect metadata that
    // makes displayWidth half the real width, which would draw every frame at half resolution.
    const { w, h } = renderSize(frame, this.expected);
    this.resize(w, h);
    // drawImage only reads a frame's visibleRect: widen it when it is wrong (see renderSize).
    const init = widenTo(frame, w, h);
    let src = frame;
    try {
      if (init) src = new VideoFrame(frame, init);
    } catch {
      // An engine that refuses the wider rect: draw what it reports.
    }
    this.timed(() => (this.gl ? this.gl.frame(src, w, h) : this.ctx!.drawImage(src, 0, 0, w, h)));
    if (src !== frame) src.close();
    const t0 = this.pending.get(frame.timestamp);
    if (t0 !== undefined) {
      this.decodeSum += performance.now() - t0;
      this.pending.delete(frame.timestamp);
    }
    frame.close();
    if (!this.pending.size && this.strips.length) this.drawExact(this.strips.splice(0));
    this.tick();
  }

  private drawRgba(p: Packet) {
    this.native = true;
    this.fed++;
    if (this.devStall === "native") return this.cb.onRgbaDrawn?.();
    this.resize(p.width, p.height);
    const px = new Uint8ClampedArray(p.data.buffer as ArrayBuffer, p.data.byteOffset, p.width * p.height * 4);
    this.timed(() => {
      if (this.gl) return this.gl.rgba(p.data.subarray(0, px.length), p.width, p.height, p.bgra);
      if (p.bgra) {
        for (let i = 0; i < px.length; i += 4) {
          const b = px[i];
          px[i] = px[i + 2];
          px[i + 2] = b;
        }
      }
      this.ctx!.putImageData(new ImageData(px, p.width, p.height), 0, 0);
    });
    this.cb.onRgbaDrawn?.();
    this.stats.codec = "native";
    this.tick();
  }

  private tick() {
    this.frames++;
    this.drawn++;
    const now = performance.now();
    if (this.lastDraw) this.maxGap = Math.max(this.maxGap, now - this.lastDraw);
    this.lastDraw = now;
    this.sample(now);
  }

  /** Refresh the per-second stats; also called on a timer so a frozen picture shows up as one. */
  sample(now = performance.now()): VideoStats {
    const dt = now - this.lastStats;
    if (dt >= 1000) {
      this.stats = {
        ...this.stats,
        fps: Math.round((this.frames * 1000) / dt),
        inFps: Math.round((this.packets * 1000) / dt),
        decodeMs: this.frames ? +(this.decodeSum / this.frames).toFixed(1) : 0,
        width: this.w,
        height: this.h,
        maxGapMs: Math.round(Math.max(this.maxGap, this.lastDraw ? now - this.lastDraw : 0)),
        keyRequests: this.key.requests,
        dropped: this.dropped,
        renderMs: this.frames ? +(this.renderSum / this.frames).toFixed(2) : 0,
        sharpen: this.gl ? `${this.sharpen} ${this.gl.amount.toFixed(2)}` : this.sharpen === "off" ? "off" : "unavailable (2D canvas)",
      };
      this.renderSum = 0;
      this.sync = syncMode();
      this.maxGap = 0;
      this.frames = 0;
      this.packets = 0;
      this.decodeSum = 0;
      this.lastStats = now;
    }
    return this.stats;
  }

  /** After a reconnect: keep the decoder and the last picture, resume at the next key frame. */
  resync() {
    // The new stream starts with a key frame; asking now would restart the host's video service
    // (~1 s). Only a late key frame triggers requests (KeyRecovery.expect + retry timer).
    this.key.expect(performance.now());
    this.startKeyRetry();
    this.pending.clear();
  }

  /** The host's size of the shown display (peer_info), used to size frames (see renderSize). */
  expect(w: number, h: number) {
    this.expected = { w, h };
  }

  /** Switch the shown display; the next key frame repaints. */
  setDisplay(d: number) {
    this.display = d;
    this.needKey("display switched");
  }

  close() {
    clearInterval(this.retryTimer);
    this.gl?.dispose();
    this.gl = null;
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
  }
}

/**
 * A VideoFrame init that makes the whole w×h top-left picture visible, or null when the frame's
 * visibleRect already is that picture. drawImage only reads visibleRect, so a wrong one showed a
 * Retina Mac's screen as its top-left corner, scaled up to the window.
 */
export function widenTo(f: { visibleRect?: { x: number; y: number; width: number; height: number } | null }, w: number, h: number) {
  const v = f.visibleRect;
  if (!v || (v.x === 0 && v.y === 0 && v.width === w && v.height === h)) return null;
  return { visibleRect: { x: 0, y: 0, width: w, height: h }, displayWidth: w, displayHeight: h };
}

/** Alignment padding a coded frame may carry beyond the picture (macroblocks, CTUs). */
const PAD = 64;

/**
 * Size to draw a decoded frame at. WebView2's hardware HEVC path can report a visibleRect that is
 * not the picture: exactly half the coded size, or a fixed 1280×720 for a Retina Mac's 3456×2234
 * stream (coded 3456×2240). The host's display size (`expected`, from peer_info) is the truth when
 * the coded frame is that size plus alignment padding. Without it: an exact 2× ratio or a visible
 * rect far smaller than the coded frame means the coded frame is the picture; small differences
 * are real cropping (1920×1088 → 1920×1080).
 */
export function renderSize(f: { codedWidth: number; codedHeight: number; visibleRect?: { width: number; height: number } | null }, expected?: { w: number; h: number } | null) {
  const { codedWidth: cw, codedHeight: ch } = f;
  if (expected && expected.w > 0 && expected.h > 0 && cw >= expected.w && ch >= expected.h && cw - expected.w < PAD && ch - expected.h < PAD) {
    return { w: expected.w, h: expected.h };
  }
  const v = f.visibleRect;
  if (!v || (v.width * 2 === cw && v.height * 2 === ch) || cw - v.width >= PAD || ch - v.height >= PAD) {
    return { w: cw, h: ch };
  }
  return { w: v.width, h: v.height };
}
