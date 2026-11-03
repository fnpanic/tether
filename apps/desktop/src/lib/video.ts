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

export interface VideoStats {
  fps: number;
  /** Encoded packets received per second (before decoding). */
  inFps: number;
  coded: string;
  decodeMs: number;
  width: number;
  height: number;
  codec: string;
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
  private waitingKey = true;
  private pending = new Map<number, number>();
  private frames = 0;
  private decodeSum = 0;
  private lastStats = performance.now();
  private w = 0;
  private h = 0;
  stats: VideoStats = { fps: 0, inFps: 0, coded: "", decodeMs: 0, width: 0, height: 0, codec: "" };
  private packets = 0;
  display = 0;

  constructor(private canvas: HTMLCanvasElement, private cb: Callbacks) {
    // desynchronized: bypass the compositor queue where supported → lower glass-to-glass latency.
    const ctx = canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) throw new Error("2d context unavailable");
    this.ctx = ctx;
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
    this.waitingKey = true;
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
    this.stats.codec = Object.entries(CODEC).find(([, v]) => v === codec)?.[0]?.toUpperCase() ?? "";
  }

  private decode(p: Packet) {
    if (!this.decoder || this.decoder.state === "closed" || p.codec !== this.codec) {
      this.configure(p.codec);
    }
    const dec = this.decoder!;
    if (this.waitingKey) {
      if (!p.key) return;
      this.waitingKey = false;
    }
    // Never let the queue grow: a backed-up decoder means stale pictures, so drop and resync.
    if (dec.decodeQueueSize > 3) {
      this.waitingKey = true;
      this.cb.onNeedKey(this.display);
      return;
    }
    const ts = p.pts * 1000;
    this.pending.set(ts, performance.now());
    try {
      dec.decode(new EncodedVideoChunk({ type: p.key ? "key" : "delta", timestamp: ts, data: p.data }));
    } catch (e) {
      console.warn("[video] decode threw", e);
      this.waitingKey = true;
      this.cb.onNeedKey(this.display);
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
    const dt = now - this.lastStats;
    if (dt >= 1000) {
      this.stats = {
        ...this.stats,
        fps: Math.round((this.frames * 1000) / dt),
        inFps: Math.round((this.packets * 1000) / dt),
        decodeMs: this.frames ? +(this.decodeSum / this.frames).toFixed(1) : 0,
        width: this.w,
        height: this.h,
      };
      this.frames = 0;
      this.packets = 0;
      this.decodeSum = 0;
      this.lastStats = now;
    }
  }

  /** Switch the shown display; the next key frame repaints. */
  setDisplay(d: number) {
    this.display = d;
    this.waitingKey = true;
  }

  close() {
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
