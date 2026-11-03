// Hardware video decoding in the webview (WebCodecs) for frames forwarded raw from the Rust core.
// Packet layout (little endian, see vendor/rustdesk/src/tauri_ui/mod.rs):
//   kind u8 | codec u8 | flags u8 (bit0 key, bit1 bgra) | display u8 | w u32 | h u32 | pts u32 | payload

export const PKT_ENCODED = 0;
export const PKT_RGBA = 1;
export const CODEC = { vp8: 1, vp9: 2, av1: 3, h264: 4, h265: 5 } as const;
export type CodecName = keyof typeof CODEC;

const CODEC_STRING: Record<number, string> = {
  1: "vp8",
  2: "vp09.00.10.08",
  3: "av01.0.08M.08",
  4: "avc1.64002A",
  5: "hvc1.1.6.L153.B0",
};

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
}

interface Callbacks {
  /** Decoder broke or rejected the stream: switch this codec to native decoding. */
  onFail: (codec: number, display: number) => void;
  /** Need a key frame (after reset or a dropped delta). */
  onNeedKey: (display: number) => void;
  onSize: (w: number, h: number) => void;
}

export class VideoPipeline {
  private ctx: CanvasRenderingContext2D;
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
  private h = 0;
  stats: VideoStats = { fps: 0, inFps: 0, coded: "", decodeMs: 0, width: 0, height: 0, codec: "", maxGapMs: 0, keyRequests: 0, dropped: 0 };
  private packets = 0;
  private decodedSinceConfigure = 0;
  display = 0;

  constructor(private canvas: HTMLCanvasElement, private cb: Callbacks) {
    // desynchronized: bypass the compositor queue where supported → lower glass-to-glass latency.
    const ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) throw new Error("2d context unavailable");
    this.ctx = ctx;
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
    if (p.kind === PKT_RGBA) return this.drawRgba(p);
    this.decode(p);
  }

  private resize(w: number, h: number) {
    if (w === this.w && h === this.h) return;
    this.w = this.canvas.width = w;
    this.h = this.canvas.height = h;
    this.cb.onSize(w, h);
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
    dec.configure({ codec: CODEC_STRING[codec], optimizeForLatency: true, hardwareAcceleration: "no-preference" });
    this.decoder = dec;
    this.decodedSinceConfigure = 0;
    this.stats.codec = Object.entries(CODEC).find(([, v]) => v === codec)?.[0]?.toUpperCase() ?? "";
  }

  private decode(p: Packet) {
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
    const ts = p.pts * 1000;
    this.pending.set(ts, performance.now());
    try {
      dec.decode(new EncodedVideoChunk({ type: p.key ? "key" : "delta", timestamp: ts, data: p.data }));
    } catch (e) {
      console.warn("[video] decode threw", e);
      this.needKey("decode threw");
    }
  }

  private output(frame: VideoFrame) {
    this.stats.coded = `${frame.codedWidth}×${frame.codedHeight}`;
    // Use the visible picture size, not displayWidth: some HEVC streams carry aspect metadata that
    // makes displayWidth half the real width, which would draw every frame at half resolution.
    const { w, h } = renderSize(frame);
    this.resize(w, h);
    this.ctx.drawImage(frame, 0, 0, w, h);
    const t0 = this.pending.get(frame.timestamp);
    if (t0 !== undefined) {
      this.decodeSum += performance.now() - t0;
      this.pending.delete(frame.timestamp);
    }
    frame.close();
    this.tick();
  }

  private drawRgba(p: Packet) {
    this.resize(p.width, p.height);
    const px = new Uint8ClampedArray(p.data.buffer as ArrayBuffer, p.data.byteOffset, p.width * p.height * 4);
    if (p.bgra) {
      for (let i = 0; i < px.length; i += 4) {
        const b = px[i];
        px[i] = px[i + 2];
        px[i + 2] = b;
      }
    }
    this.ctx.putImageData(new ImageData(px, p.width, p.height), 0, 0);
    this.stats.codec = "native";
    this.tick();
  }

  private tick() {
    this.frames++;
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
      };
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

  /** Switch the shown display; the next key frame repaints. */
  setDisplay(d: number) {
    this.display = d;
    this.needKey("display switched");
  }

  close() {
    clearInterval(this.retryTimer);
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
  }
}

/**
 * Size to draw a decoded frame at. WebView2's hardware HEVC path reports visibleRect/displayWidth
 * as exactly half the coded size while the picture actually fills the coded frame; trusting it
 * would downscale every frame to a quarter of its pixels. Real cropping (alignment padding such
 * as 1920×1088 → 1920×1080) is never an exact 2× ratio, so it keeps using visibleRect.
 */
export function renderSize(f: { codedWidth: number; codedHeight: number; visibleRect?: { width: number; height: number } | null }) {
  const v = f.visibleRect;
  if (!v || (v.width * 2 === f.codedWidth && v.height * 2 === f.codedHeight)) {
    return { w: f.codedWidth, h: f.codedHeight };
  }
  return { w: v.width, h: v.height };
}
