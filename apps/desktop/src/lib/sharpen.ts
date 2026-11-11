// Client-side sharpening of the remote picture: scaling to the window and an adaptive, clamped luma
// unsharp mask in one WebGL2 pass (see docs/performance.md, "Sharpening"). The 2D canvas path in
// video.ts stays the fallback (setting Off, no WebGL2, context lost).

export type SharpenMode = "off" | "auto" | "strong";
export const SHARPEN_MODES: SharpenMode[] = ["off", "auto", "strong"];
const KEY = "tether.sharpen";

/** The user's choice, per OS user (webview storage); Auto when unset or unreadable. */
export function loadSharpen(): SharpenMode {
  try {
    const v = localStorage.getItem(KEY);
    return v === "off" || v === "strong" ? v : "auto";
  } catch {
    return "auto";
  }
}

export function saveSharpen(m: SharpenMode) {
  try {
    localStorage.setItem(KEY, m);
  } catch {}
}

/** Shader strengths (k in the shader), chosen with e2e/sharpen-quality.mjs (docs/performance.md).
 * 0.2 on a scaled picture brings the text sharpness of soft pictures (key frames, H.264 at 1 Mbps)
 * to that of the ideally scaled original and lowers their error; well-coded ones gain ≤ 10 %. */
export const AMOUNT = { scaled: 0.2, strong: 0.5 };

type Size = { w: number; h: number };

/**
 * Sharpening strength for a picture of size `src` drawn at `dst` device pixels. Auto sharpens what
 * scaling softens and skips 1:1, where it measured neutral on average (better for soft key frames,
 * worse for a static screen the codec has refined), so it is not worth any risk of artifacts.
 */
export function sharpenAmount(mode: SharpenMode, src: Size, dst: Size): number {
  if (mode === "off" || src.w <= 0 || src.h <= 0 || dst.w <= 0 || dst.h <= 0) return 0;
  if (mode === "strong") return AMOUNT.strong;
  return Math.abs(dst.w - src.w) <= 1 && Math.abs(dst.h - src.h) <= 1 ? 0 : AMOUNT.scaled;
}

/** Distance of the neighbour taps in source texels: one output pixel when shrinking, one texel otherwise. */
export function tapStep(src: Size, dst: Size): number {
  return dst.w > 0 ? Math.max(1, src.w / dst.w) : 1;
}

const VS = `#version 300 es
out vec2 uv;
void main() {
  vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
  uv = vec2(p.x, 1.0 - p.y);
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;

// Unsharp mask on luma (no colour fringes) from a 5-tap cross, applied only where the local
// contrast is above compression noise (noise gate) and clamped per channel to the min/max of the
// taps, so it can never overshoot the neighbourhood: no halos or ringing.
const FS = `#version 300 es
precision highp float;
uniform sampler2D t;
uniform vec2 d;
uniform float k;
uniform bool bgra;
in vec2 uv;
out vec4 o;
const vec3 Y = vec3(0.299, 0.587, 0.114);
void main() {
  vec3 c = texture(t, uv).rgb;
  if (k > 0.0) {
    vec3 n = texture(t, uv - vec2(0.0, d.y)).rgb;
    vec3 s = texture(t, uv + vec2(0.0, d.y)).rgb;
    vec3 w = texture(t, uv - vec2(d.x, 0.0)).rgb;
    vec3 e = texture(t, uv + vec2(d.x, 0.0)).rgb;
    vec3 mn = min(c, min(min(n, s), min(w, e)));
    vec3 mx = max(c, max(max(n, s), max(w, e)));
    float lc = dot(c, Y);
    float lap = lc - 0.25 * (dot(n, Y) + dot(s, Y) + dot(w, Y) + dot(e, Y));
    float gate = smoothstep(0.02, 0.08, dot(mx - mn, Y));
    c = clamp(c + k * gate * lap, mn, mx);
  }
  o = vec4(bgra ? c.bgr : c, 1.0);
}`;

/** Read the device-pixel size of the canvas (pixel exact in Chromium; rounded CSS size × DPR elsewhere). */
function observe(canvas: HTMLCanvasElement, set: (w: number, h: number) => void): ResizeObserver | null {
  if (typeof ResizeObserver === "undefined") return null;
  const ro = new ResizeObserver(([e]) => {
    const dp = e.devicePixelContentBoxSize?.[0];
    if (dp) set(dp.inlineSize, dp.blockSize);
    else set(Math.round(e.contentRect.width * devicePixelRatio), Math.round(e.contentRect.height * devicePixelRatio));
  });
  try {
    ro.observe(canvas, { box: "device-pixel-content-box" });
  } catch {
    ro.observe(canvas); // WebKit: no device-pixel-content-box
  }
  return ro;
}

export class GlRenderer {
  mode: SharpenMode = "auto";
  /** Strength used for the last draw (shown in the statistics). */
  amount = 0;
  src = { w: 0, h: 0 };
  private bgra = false;
  private ro: ResizeObserver | null = null;
  private u: { d: WebGLUniformLocation | null; k: WebGLUniformLocation | null; bgra: WebGLUniformLocation | null };
  private px = new Uint8Array(4);

  private constructor(
    private canvas: HTMLCanvasElement,
    readonly gl: WebGL2RenderingContext,
    private prog: WebGLProgram,
    private tex: WebGLTexture,
    private onLost: () => void,
  ) {
    this.u = { d: gl.getUniformLocation(prog, "d"), k: gl.getUniformLocation(prog, "k"), bgra: gl.getUniformLocation(prog, "bgra") };
    canvas.addEventListener("webglcontextlost", this.lost);
  }

  /** null when WebGL2 or the shader is unavailable (the caller uses the 2D canvas). `track`: size the
   * canvas to its on-screen device pixels (off for offscreen tests, which call setOutput). */
  static create(canvas: HTMLCanvasElement, onLost: () => void, track = true): GlRenderer | null {
    let gl: WebGL2RenderingContext | null = null;
    try {
      // preserveDrawingBuffer: the picture stays readable (drawImage of the canvas: stress sampler,
      // mode switch) after it was composited; measured at no cost (docs/performance.md).
      gl = canvas.getContext("webgl2", { alpha: false, antialias: false, depth: false, stencil: false, desynchronized: true, preserveDrawingBuffer: true });
    } catch {}
    if (!gl || gl.isContextLost()) return null;
    const sh = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, sh(gl.VERTEX_SHADER, VS));
    gl.attachShader(prog, sh(gl.FRAGMENT_SHADER, FS));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
      console.warn("[video] sharpen shader failed", gl.getProgramInfoLog(prog));
      return null;
    }
    gl.useProgram(prog);
    const tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, tex);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
    const r = new GlRenderer(canvas, gl, prog, tex, onLost);
    if (track) r.ro = observe(canvas, (w, h) => r.setOutput(w, h));
    return r;
  }

  private lost = () => {
    console.warn("[video] WebGL context lost: drawing with the 2D canvas");
    this.onLost();
  };

  /** Canvas size in device pixels; redraws the current picture at the new size. */
  setOutput(w: number, h: number) {
    if (w === this.canvas.width && h === this.canvas.height) return;
    this.canvas.width = w;
    this.canvas.height = h;
    this.draw();
  }

  /** Upload a picture (a VideoFrame without a copy through the CPU) and draw it. */
  frame(src: TexImageSource, w: number, h: number) {
    const gl = this.gl;
    if (gl.isContextLost()) return;
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, src);
    this.src = { w, h };
    this.bgra = false;
    this.draw();
  }

  /** Exact rows (RGBA) written into the picture on screen from row `y`; the next picture replaces
   * it whole. The caller draws (`draw`) after its last strip. */
  strip(data: Uint8Array, y: number, w: number, h: number) {
    const gl = this.gl;
    if (gl.isContextLost() || this.bgra || w !== this.src.w || y + h > this.src.h) return;
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, y, w, h, gl.RGBA, gl.UNSIGNED_BYTE, data);
  }

  /** A natively decoded picture; BGRA is swizzled in the shader instead of in JS. */
  rgba(data: Uint8Array, w: number, h: number, bgra: boolean) {
    const gl = this.gl;
    if (gl.isContextLost()) return;
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, data);
    this.src = { w, h };
    this.bgra = bgra;
    this.draw();
  }

  draw() {
    const gl = this.gl;
    const { w, h } = this.src;
    if (!w || !h || !this.canvas.width || gl.isContextLost()) return;
    const dst = { w: this.canvas.width, h: this.canvas.height };
    this.amount = sharpenAmount(this.mode, this.src, dst);
    const step = tapStep(this.src, dst);
    gl.viewport(0, 0, dst.w, dst.h);
    gl.uniform2f(this.u.d, step / w, step / h);
    gl.uniform1f(this.u.k, this.amount);
    gl.uniform1i(this.u.bgra, this.bgra ? 1 : 0);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  /** Wait until the GPU finished (debug measurements only: stalls the pipeline). */
  sync() {
    this.gl.readPixels(0, 0, 1, 1, this.gl.RGBA, this.gl.UNSIGNED_BYTE, this.px);
  }

  /** The current source picture (RGBA, top row first), to carry it over to another canvas. */
  snapshot(): ImageData | null {
    const gl = this.gl;
    const { w, h } = this.src;
    if (!w || !h || gl.isContextLost()) return null;
    const fb = gl.createFramebuffer();
    gl.bindFramebuffer(gl.FRAMEBUFFER, fb);
    gl.framebufferTexture2D(gl.FRAMEBUFFER, gl.COLOR_ATTACHMENT0, gl.TEXTURE_2D, this.tex, 0);
    const px = new Uint8ClampedArray(w * h * 4);
    gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, px);
    gl.bindFramebuffer(gl.FRAMEBUFFER, null);
    gl.deleteFramebuffer(fb);
    if (this.bgra) for (let i = 0; i < px.length; i += 4) [px[i], px[i + 2]] = [px[i + 2], px[i]];
    return new ImageData(px, w, h);
  }

  /** `release`: the canvas is discarded, free its GPU context now instead of at garbage collection
   * (browsers cap live contexts). Not when the canvas stays (a new pipeline may draw into it). */
  dispose(release = false) {
    this.ro?.disconnect();
    this.canvas.removeEventListener("webglcontextlost", this.lost);
    const gl = this.gl;
    gl.deleteTexture(this.tex);
    gl.deleteProgram(this.prog);
    if (release) gl.getExtension("WEBGL_lose_context")?.loseContext();
  }
}
