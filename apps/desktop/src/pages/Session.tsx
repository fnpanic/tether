import { useCallback, useEffect, useRef, useState } from "react";
import { routeLabel } from "../lib/route";
import {
  Activity,
  Check,
  ChevronDown,
  Expand,
  FolderSync,
  Gauge,
  Keyboard,
  Maximize,
  MessageSquare,
  Minimize,
  Minus,
  MonitorCog,
  MonitorSmartphone,
  Pin,
  PinOff,
  Power,
  Scan,
  Send,
  ShieldCheck,
  Video,
  X,
} from "lucide-react";
import { api, formatId, invoke, subscribe, videoChannel } from "../lib/ipc";
import { probeCodecs, VideoPipeline, type VideoStats } from "../lib/video";
import { loadSharpen, saveSharpen, SHARPEN_MODES, type SharpenMode } from "../lib/sharpen";
import { Button, IconButton, Logo, TitleBar, useToast } from "../components/ui";
import { RecordingSaved } from "../components/RecordingSaved";
import { FileTransfer } from "./FileTransfer";
import { mouseMask, MOUSE, wheelSteps, lockModes } from "../lib/input";
import { classify, onFailure, reconnectMinutes, type Series } from "../lib/reconnect";
import { passwordWhy, RESUME_GRACE_MS } from "../lib/login";

interface Display {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface PeerInfo {
  hostname: string;
  username: string;
  platform: string;
  displays: Display[];
  current_display: number;
  /** The remote side runs as a service (SYSTEM): Ctrl+Alt+Del, lock screen and UAC work. */
  sas_enabled?: boolean;
}

interface Quality {
  fps?: Record<string, number>;
  delay?: number;
  target_bitrate?: number;
  codec?: string;
  speed?: string;
}

type Phase =
  | { kind: "connecting"; text: string }
  | { kind: "password"; wrong: boolean; why: string | null }
  | { kind: "connected" }
  | { kind: "reconnecting"; attempt: number; text: string }
  | { kind: "error"; title: string; text: string; retry: boolean };

/** idle → pending (asked the host user) → on (core recorder running). */
type Rec = { state: "idle" | "pending" | "on"; since: number };

type Fit = "fit" | "original";

const QUALITY = [
  { value: "best", label: "Sharp text", hint: "Best image quality" },
  { value: "balanced", label: "Balanced", hint: "Quality and speed" },
  { value: "low", label: "Speed", hint: "Lowest latency and bandwidth" },
];

const SHARPEN: Record<SharpenMode, { label: string; hint: string }> = {
  off: { label: "Off", hint: "Picture as received" },
  auto: { label: "Auto", hint: "Crisper text when scaled or compressed" },
  strong: { label: "Strong", hint: "Maximum crispness" },
};

const stored = (k: string) => {
  try {
    return sessionStorage.getItem(k) === "1";
  } catch {
    return false;
  }
};
const store = (k: string) => {
  try {
    sessionStorage.setItem(k, "1");
  } catch {}
};

/** Human wording for the core's msgbox texts. */
function friendly(text: string): string {
  const t = text.toLowerCase();
  if (t.includes("offline")) return "The remote device is offline. Make sure Tether is running on it.";
  if (t.includes("not exist") || t.includes("id does not exist")) return "No device with this ID. Check the number and try again.";
  if (t.includes("timeout") || t.includes("timed out")) return "The connection timed out. Check both devices' internet connection.";
  if (t.includes("reset by the peer") || t.includes("closed")) return "The remote side ended the session.";
  if (t.includes("rejected") || t.includes("denied")) return "The remote user declined the request.";
  return text;
}

export function Session({ sessionKey, peer, conn, viewOnly = false }: { sessionKey: string; peer: string; conn: string; viewOnly?: boolean }) {
  if (conn === "file-transfer") return <FileTransfer sessionKey={sessionKey} peer={peer} />;
  return <RemoteView sessionKey={sessionKey} peer={peer} viewOnly={viewOnly} />;
}

/** `viewOnly`: opened as "View only" (Advanced mode, `--view-only`): the core's per-peer view-only
 *  option is switched on for this session (the host then refuses input) and back off when it ends. */
function RemoteView({ sessionKey: key, peer, viewOnly }: { sessionKey: string; peer: string; viewOnly: boolean }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const pipeRef = useRef<VideoPipeline | null>(null);
  const [phase, setPhase] = useState<Phase>({ kind: "connecting", text: "Connecting…" });
  const [info, setInfo] = useState<PeerInfo | null>(null);
  const [display, setDisplay] = useState<Display>({ x: 0, y: 0, width: 1920, height: 1080 });
  const [displayIdx, setDisplayIdx] = useState(0);
  const [frameSize, setFrameSize] = useState({ w: 0, h: 0 });
  const [fit, setFit] = useState<Fit>("fit");
  const [quality, setQuality] = useState<Quality>({});
  const [vstats, setVstats] = useState<VideoStats | null>(null);
  const [route, setRoute] = useState<string | null>(null);
  const [secured, setSecured] = useState<boolean | null>(null);
  const [fingerprint, setFingerprint] = useState("");
  const [showStats, setShowStats] = useState(false);
  const [toolbar, setToolbar] = useState(true);
  const barRef = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(false);
  const [chat, setChat] = useState<{ open: boolean; unread: number; msgs: { me: boolean; text: string }[] }>({ open: false, unread: 0, msgs: [] });
  const [cursor, setCursor] = useState("default");
  const [kbd, setKbd] = useState<KbdSettings | null>(null);
  // The person at the remote computer turned "Control mouse and keyboard" off.
  const [controlOff, setControlOff] = useState(false);
  const [imageQuality, setImageQuality] = useState("balanced");
  const [sharpen, setSharpenState] = useState<SharpenMode>(loadSharpen);
  // WebGL unavailable or its context lost: draw with the 2D canvas for the rest of this window.
  const [glFailed, setGlFailed] = useState(false);
  const useGl = sharpen !== "off" && !glFailed;
  const glRef = useRef({ sharpen, useGl });
  const [fullscreen, setFullscreen] = useState(false);
  const cursors = useRef(new Map<string, string>());
  const displayRef = useRef(display);
  displayRef.current = display;
  // The host's display size sizes the picture when the decoder reports a wrong visible rect.
  useEffect(() => pipeRef.current?.expect(display.width, display.height), [display]);
  // Fit: scale the picture to the stage, up as well as down (a small remote screen fills the window).
  const [stage, setStage] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setStage({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const displayIdxRef = useRef(displayIdx);
  displayIdxRef.current = displayIdx;
  // Reconnect: the running series, its timer and the policy limit (event handlers read refs).
  const series = useRef<Series | null>(null);
  const retryTimer = useRef(0);
  const restoreView = useRef(false);
  const limitMin = useRef(10);
  // Survives a reload of this window (F5), which continues the same session.
  const everConnected = useRef(stored(`connected-${key}`));
  // A password typed in this window (only then can a rejection say "wrong"), and the delayed screen.
  const typed = useRef(false);
  const pwTimer = useRef(0);
  // Recording: policy mode and state (mirrored in a ref for the event listener).
  const [recMode, setRecMode] = useState("off");
  const recModeRef = useRef("off");
  const [rec, setRecState] = useState<Rec>({ state: "idle", since: 0 });
  const recRef = useRef(rec);
  const setRec = (r: Rec) => {
    recRef.current = r;
    setRecState(r);
  };
  const toast = useToast();
  /** Every toolbar action either works or says why not. */
  const act = (p: Promise<unknown>) => p.catch((e) => toast.show(String(e)));
  const [ask, setAsk] = useState<"lock" | "restart" | null>(null);
  /** What the host said is missing (a Mac without Screen Recording / Accessibility). */
  const [hostMissing, setHostMissing] = useState<string[]>([]);

  // ---------------------------------------------------------------- connection + events
  useEffect(() => {
    let alive = true;
    const pipe = new VideoPipeline(canvasRef.current!, {
      onFail: (codec, d) => invoke("session_webcodecs_failed", { key, codec, display: d }),
      onNeedKey: (d) => invoke("session_refresh", { key, display: d }).catch(() => {}),
      onSize: (w, h) => setFrameSize({ w, h }),
      onRgbaDrawn: () => invoke("session_frame_drawn", { key }).catch(() => {}),
      onGlLost: () => setGlFailed(true),
    }, glRef.current.sharpen, glRef.current.useGl);
    if (glRef.current.useGl && !pipe.webgl) setGlFailed(true);
    pipeRef.current = pipe;
    const sub = subscribe("session", (e) => {
      const p = e.payload;
      switch (e.name) {
        case "msgbox":
          onMsgbox(p.type, p.title, p.text, p.retry);
          break;
        case "peer_info":
          setInfo(p);
          setControlOff(false); // new connection: the host resends it if control is off
          setWindowTitle(`${p.hostname || formatId(peer)} — Tether`);
          invoke("session_apply_fluxrt", { key });
          if (p.displays?.length) {
            const i = Math.min(p.current_display ?? 0, p.displays.length - 1);
            setDisplayIdx(i);
            setDisplay(p.displays[i]);
          }
          // Reconnected: same decoder and canvas; the new stream starts with a key frame.
          if (series.current) pipe.resync();
          series.current = null;
          everConnected.current = true;
          store(`connected-${key}`);
          typed.current = false;
          clearTimeout(pwTimer.current);
          setPhase({ kind: "connected" });
          // Policy "always": ask right away; the host user still has to allow it.
          if (recModeRef.current === "always" && recRef.current.state === "idle") startRecording();
          break;
        case "record_status":
          if (p.start) {
            setRec({ state: "on", since: Date.now() });
            // A recording file has to start with a key frame.
            invoke("session_refresh", { key, display: displayIdxRef.current });
          } else recordingStopped();
          break;
        case "permission":
          if (p.name === "keyboard") setControlOff(!p.value);
          if (p.name !== "recording" || recRef.current.state !== "pending") break;
          // The host user answered our request (see cm_record_consent).
          if (p.value) invoke("session_record", { key, start: true });
          else {
            setRec({ state: "idle", since: 0 });
            toast.show("Recording declined by the other party");
          }
          break;
        case "displays":
          setInfo((i) => (i ? { ...i, displays: p.displays } : i));
          break;
        case "switch_display":
          setDisplayIdx(p.display);
          setDisplay({ x: p.x, y: p.y, width: p.width, height: p.height });
          pipeRef.current?.setDisplay(p.display);
          break;
        case "close_success":
          setPhase({ kind: "connected" });
          break;
        case "connection_type":
          setRoute(routeLabel(p.direct, p.stream_type));
          setSecured(p.secured);
          break;
        case "fingerprint":
          setFingerprint(p.fingerprint ?? "");
          break;
        case "quality":
          setQuality((q) => ({ ...q, ...Object.fromEntries(Object.entries(p).filter(([, v]) => v != null)) }));
          break;
        case "cursor_data": {
          const css = `url(data:image/png;base64,${p.png}) ${p.hotx} ${p.hoty}, default`;
          cursors.current.set(p.id, css);
          setCursor(css);
          break;
        }
        case "cursor_id":
          setCursor(cursors.current.get(p.id) ?? "default");
          break;
        case "chat":
          setChat((c) => ({ ...c, unread: c.open ? 0 : c.unread + 1, msgs: [...c.msgs, { me: false, text: p.text }] }));
          break;
      }
    });
    (async () => {
      const formats = await probeCodecs();
      limitMin.current = reconnectMinutes(await invoke<string>("session_policy_get", { id: "connect.reconnect-minutes" }).catch(() => ""));
      const mode = (await invoke<string>("session_policy_get", { id: "recording.mode" }).catch(() => "")) || "off";
      recModeRef.current = mode;
      setRecMode(mode);
      // Uploads left over from a closed window or an earlier run.
      if (mode !== "off") invoke("recording_finished").catch(() => {});
      await sub.ready;
      // StrictMode's discarded first run must not start (and consume the pending login of) a session.
      if (!alive) return;
      const ch = videoChannel((buf) => pipe.push(buf));
      await invoke("session_start", { key, peer, webcodecs: formats, video: ch });
      if (viewOnly && !(await invoke<boolean>("session_get_toggle", { key, name: "view-only" }).catch(() => true))) {
        await invoke("session_toggle", { key, name: "view-only" });
        restoreView.current = true;
      }
      setImageQuality((await invoke<string>("get_option", { key: "image-quality" }).catch(() => "")) || "balanced");
    })().catch((e) => alive && setPhase({ kind: "error", title: "Could not start the session", text: String(e), retry: false }));
    const t = setInterval(() => pipeRef.current && setVstats({ ...pipeRef.current.sample() }), 1000);
    return () => {
      alive = false;
      sub.off();
      clearInterval(t);
      clearTimeout(retryTimer.current);
      clearTimeout(pwTimer.current);
      if (recRef.current.state === "on") invoke("recording_finished").catch(() => {});
      pipe.close();
      // Same IPC order: the option is back to the user's choice before the session goes.
      if (restoreView.current) invoke("session_toggle", { key, name: "view-only" }).catch(() => {});
      restoreView.current = false;
      invoke("session_close", { key });
    };
  }, [key, peer]); // eslint-disable-line react-hooks/exhaustive-deps

  // Sharpening: Off draws with the 2D canvas, Auto/Strong with WebGL; a switch remounts the canvas
  // (key below), whose new element the pipeline takes over with the current picture.
  useEffect(() => {
    const p = pipeRef.current;
    if (!p) return;
    p.setSharpen(sharpen);
    if (!p.attach(canvasRef.current!, useGl) && useGl) setGlFailed(true);
  }, [sharpen, useGl]);

  const onMsgbox = (type: string, title: string, text: string, retry: boolean) => {
    // Sent by a Mac host that lacks permissions (the session itself keeps running).
    if (type === "tether-mac-permission") {
      const missing = title.split(",");
      setHostMissing(missing);
      if (!missing.includes("screen-recording")) toast.show("The Mac ignores mouse and keyboard: Accessibility is not allowed for Tether there (see the help in the window).");
      return;
    }
    if (type === "connecting" && series.current) return; // keep the reconnect overlay
    if (type === "input-password" || type === "re-input-password") {
      const show = () => {
        series.current = null;
        clearTimeout(retryTimer.current);
        setPhase({ kind: "password", wrong: type === "re-input-password" && typed.current, why: passwordWhy(text, everConnected.current) });
      };
      clearTimeout(pwTimer.current);
      // Reconnecting a session accepted by click: the host lets this client back in by itself.
      if (everConnected.current && type === "input-password" && !text) pwTimer.current = window.setTimeout(show, RESUME_GRACE_MS);
      else show();
      return;
    }
    // The host dropped a click or key aimed at its own Tether window (only its user may use it).
    if (type === "info" && title === "tether-own-window") return toast.show(text, 4000);
    if (type === "connecting") return setPhase({ kind: "connecting", text: friendly(text) || "Connecting…" });
    if (type === "success" || type === "info" || type === "nocancel-info") return;
    if (text.toLowerCase().includes("waiting") || type.includes("wait")) return setPhase({ kind: "connecting", text: "Waiting for the person at the remote computer to click Accept…" });
    // Only a session that was working reconnects on its own; a first connect shows the error.
    if (everConnected.current) {
      const step = onFailure(series.current, classify({ type, title, text, retry }), Date.now(), limitMin.current);
      if (step.kind === "retry") {
        if (!series.current) {
          // The old connection's recorder is gone; a new one needs consent again.
          if (recRef.current.state === "on") recordingStopped();
          else setRec({ state: "idle", since: 0 });
        }
        series.current = step.series;
        clearTimeout(retryTimer.current);
        retryTimer.current = window.setTimeout(() => invoke("session_reconnect", { key, forceRelay: step.forceRelay }), step.delayMs);
        return setPhase({ kind: "reconnecting", attempt: step.series.attempt, text: friendly(text) });
      }
    }
    const gaveUp = series.current != null;
    series.current = null;
    setPhase({ kind: "error", title: title || "Connection failed", text: friendly(text), retry: retry || gaveUp });
  };

  const startRecording = () => {
    setRec({ state: "pending", since: 0 });
    invoke("session_record", { key, start: true }).catch((e) => {
      setRec({ state: "idle", since: 0 });
      toast.show(String(e));
    });
  };

  const recordingStopped = () => {
    if (recRef.current.state === "on") invoke("recording_finished").catch(() => {});
    setRec({ state: "idle", since: 0 });
  };

  const toggleRecording = () => {
    if (rec.state === "idle") return startRecording();
    // Stopping (or withdrawing a pending request) is always allowed.
    act(invoke("session_record", { key, start: false }));
    if (rec.state === "pending") setRec({ state: "idle", since: 0 });
  };

  // ---------------------------------------------------------------- input
  const toRemote = useCallback((e: { clientX: number; clientY: number }) => {
    const c = canvasRef.current!;
    const r = c.getBoundingClientRect();
    const d = displayRef.current;
    const x = d.x + ((e.clientX - r.left) / r.width) * d.width;
    const y = d.y + ((e.clientY - r.top) / r.height) * d.height;
    return { x: Math.round(x), y: Math.round(y) };
  }, []);

  useEffect(() => {
    const c = canvasRef.current;
    if (!c || phase.kind !== "connected") return;
    let pending: PointerEvent | null = null;
    let raf = 0;
    let wheelAcc = { x: 0, y: 0 };
    const mods = (e: MouseEvent | KeyboardEvent) => ({ alt: e.altKey, ctrl: e.ctrlKey, shift: e.shiftKey, command: e.metaKey });
    const send = (mask: number, e: MouseEvent, xy = toRemote(e)) => !viewOnly && invoke("session_mouse", { key, mask, ...xy, ...mods(e) });

    // Coalesce moves to one per frame; button events go out immediately.
    const flush = () => {
      raf = 0;
      if (pending) send(MOUSE.MOVE, pending);
      pending = null;
    };
    const move = (e: PointerEvent) => {
      pending = e;
      if (!raf) raf = requestAnimationFrame(flush);
    };
    // A press or release goes out after the last position, never before it.
    const flushNow = () => {
      cancelAnimationFrame(raf);
      flush();
    };
    const down = (e: PointerEvent) => {
      flushNow();
      c.setPointerCapture(e.pointerId);
      c.focus();
      // A click into the picture always gives the session the keyboard (ends a local escape).
      invoke("session_keyboard", { key, pause: false }).catch(() => {});
      send(mouseMask(MOUSE.DOWN, e.button), e);
    };
    const up = (e: PointerEvent) => {
      flushNow();
      send(mouseMask(MOUSE.UP, e.button), e);
    };
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      const s = wheelSteps(e, wheelAcc);
      wheelAcc = s.acc;
      if ((s.x || s.y) && !viewOnly) invoke("session_mouse", { key, mask: MOUSE.WHEEL, x: s.x, y: s.y, ...mods(e) });
    };
    const ctx = (e: MouseEvent) => e.preventDefault();
    const key_ = (down: boolean) => (e: KeyboardEvent) => {
      if (chat.open && (e.target as HTMLElement)?.tagName === "INPUT") return;
      e.preventDefault();
      if (!viewOnly) invoke("session_key", { key, code: e.code, down, lockModes: lockModes(e), character: e.key.length === 1 ? e.key : "" });
    };
    const kd = key_(true);
    const ku = key_(false);
    // Text fields (chat) get the keys: the system-key hook pauses and held keys are released.
    const isText = (t: EventTarget | null) => t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement;
    const fin = (e: FocusEvent) => {
      if (isText(e.target)) invoke("session_keyboard", { key, pause: true }).catch(() => {});
    };
    const fout = (e: FocusEvent) => {
      if (isText(e.target) && !isText(e.relatedTarget)) invoke("session_keyboard", { key, pause: false }).catch(() => {});
    };
    document.addEventListener("focusin", fin);
    document.addEventListener("focusout", fout);
    c.addEventListener("pointermove", move);
    c.addEventListener("pointerdown", down);
    c.addEventListener("pointerup", up);
    c.addEventListener("wheel", wheel, { passive: false });
    c.addEventListener("contextmenu", ctx);
    window.addEventListener("keydown", kd);
    window.addEventListener("keyup", ku);
    return () => {
      cancelAnimationFrame(raf);
      c.removeEventListener("pointermove", move);
      c.removeEventListener("pointerdown", down);
      c.removeEventListener("pointerup", up);
      c.removeEventListener("wheel", wheel);
      c.removeEventListener("contextmenu", ctx);
      window.removeEventListener("keydown", kd);
      window.removeEventListener("keyup", ku);
      document.removeEventListener("focusin", fin);
      document.removeEventListener("focusout", fout);
    };
  }, [phase.kind, key, toRemote, chat.open, useGl, viewOnly]);

  // Keyboard settings; asked again once the peer's platform is known (PC/Mac mapping).
  useEffect(() => {
    if (phase.kind !== "connected") return;
    invoke<KbdSettings>("session_keyboard", { key }).then(setKbd).catch(() => {});
  }, [phase.kind, key, info?.platform]);
  const setKeyboard = (v: Partial<Record<"shortcuts" | "systemKeys" | "altTabRemote", boolean>>) =>
    act(invoke<KbdSettings>("session_keyboard", { key, ...v }).then(setKbd));

  // Auto-hide the toolbar unless pinned. Only the small handle at the top centre brings it back
  // (hover ~300 ms or click): the top edge belongs to the remote picture (title bars, tabs).
  const reveal = useRef(() => {});
  useEffect(() => {
    if (pinned || phase.kind !== "connected") return setToolbar(true);
    const hide = () => {
      t = 0;
      setToolbar(false);
    };
    let t = window.setTimeout(hide, 2200);
    reveal.current = () => {
      setToolbar(true);
      clearTimeout(t);
      t = window.setTimeout(hide, 2200); // time to reach the bar
    };
    const m = (e: MouseEvent) => {
      // Over the bar or one of its open menus it stays up (a menu must not vanish under the
      // pointer); once the pointer has left it, it hides shortly after.
      if (barRef.current?.contains(e.target as Node)) {
        clearTimeout(t);
        t = -1;
      } else if (t === -1) t = window.setTimeout(hide, 800);
    };
    window.addEventListener("mousemove", m);
    return () => {
      clearTimeout(t);
      reveal.current = () => {};
      window.removeEventListener("mousemove", m);
    };
  }, [pinned, phase.kind]);
  const hoverTimer = useRef(0);
  useEffect(() => () => clearTimeout(hoverTimer.current), []);

  const toggleFullscreen = () =>
    act(
      (async () => {
        const w = (await import("@tauri-apps/api/window")).getCurrentWindow();
        const f = !(await w.isFullscreen());
        await w.setFullscreen(f);
        setFullscreen(f);
      })(),
    );

  // Leave full screen first: a window minimized from full screen does not restore cleanly (and
  // macOS cannot minimize one at all). It goes back to full screen when it is restored.
  const refullscreen = useRef(false);
  const minimize = () =>
    act(
      (async () => {
        const w = (await import("@tauri-apps/api/window")).getCurrentWindow();
        if (await w.isFullscreen()) {
          await w.setFullscreen(false);
          setFullscreen(false);
          refullscreen.current = true;
        }
        await w.minimize();
      })(),
    );
  // Restored (taskbar, Dock or unminimize) = focused again and no longer minimized.
  useEffect(() => {
    let off = () => {};
    let gone = false;
    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => {
        const w = getCurrentWindow();
        return w.onFocusChanged(async ({ payload }) => {
          if (!payload || !refullscreen.current || (await w.isMinimized())) return;
          refullscreen.current = false;
          await w.setFullscreen(true);
          setFullscreen(true);
        });
      })
      .then((u) => (gone ? u() : (off = u)))
      .catch(() => {}); // outside Tauri
    return () => {
      gone = true;
      off();
    };
  }, []);

  const switchDisplay = (i: number) => {
    act(invoke("session_switch_display", { key, display: i }));
    pipeRef.current?.setDisplay(i);
    setDisplayIdx(i);
    if (info?.displays[i]) setDisplay(info.displays[i]);
  };

  const setQ = (v: string) => {
    setImageQuality(v);
    act(invoke("session_quality", { key, quality: v }));
  };

  const name = info?.hostname || formatId(peer);
  const connected = phase.kind === "connected";
  const delay = quality.delay;
  const latencyColor = delay == null ? "bg-muted" : delay < 40 ? "bg-ok" : delay < 120 ? "bg-warn" : "bg-danger";

  return (
    <div className="relative flex h-full flex-col bg-black text-white">
      {/* stage */}
      <div ref={stageRef} className={`relative min-h-0 flex-1 ${fit === "original" ? "overflow-auto" : "overflow-hidden"} grid place-items-center`}>
        <canvas
          key={useGl ? "gl" : "2d"}
          ref={canvasRef}
          tabIndex={0}
          style={{
            cursor,
            ...(fit === "fit"
              ? fitSize(frameSize, stage)
              : { width: frameSize.w / devicePixelRatio, height: frameSize.h / devicePixelRatio }),
            opacity: connected && frameSize.w ? 1 : phase.kind === "reconnecting" ? 0.35 : 0,
          }}
          className="block outline-none transition-opacity duration-300"
        />
        {phase.kind === "reconnecting" ? (
          <ReconnectOverlay attempt={phase.attempt} text={phase.text} />
        ) : phase.kind !== "connected" || !frameSize.w ? (
          <Overlay phase={phase} name={name} peer={peer} sessionKey={key} hostMissing={hostMissing} onLogin={() => (typed.current = true)} />
        ) : null}
        {rec.state === "on" && connected && <RecPill since={rec.since} />}
        {controlOff && connected && (
          <div role="status" className="pointer-events-none absolute bottom-4 left-1/2 z-10 -translate-x-1/2 rounded-full border border-white/10 bg-black/75 px-4 py-2 text-footnote text-white/85 backdrop-blur">
            View only: the person at the remote computer has turned off control of mouse and keyboard.
          </div>
        )}
        {toast.node}
        <RecordingSaved />
      </div>

      {/* toolbar handle: the only place that reveals a hidden toolbar */}
      {connected && !!frameSize.w && !(toolbar || chat.unread > 0) && (
        <button
          aria-label="Show toolbar"
          title="Show toolbar"
          // A held button is a drag on the remote picture, not a request for the toolbar.
          onPointerMove={(e) => {
            if (!e.buttons && !hoverTimer.current) hoverTimer.current = window.setTimeout(() => {
                hoverTimer.current = 0;
                reveal.current();
              }, 300);
          }}
          onPointerLeave={() => {
            clearTimeout(hoverTimer.current);
            hoverTimer.current = 0;
          }}
          onClick={() => reveal.current()}
          onFocus={() => reveal.current()}
          className="animate-fade absolute left-1/2 top-0 z-20 grid h-3.5 w-16 -translate-x-1/2 place-items-center rounded-b-lg border border-t-0 border-white/10 bg-black/45 text-white/60 backdrop-blur transition hover:h-5 hover:bg-black/70 hover:text-white"
        >
          <ChevronDown size={12} />
        </button>
      )}

      {/* floating toolbar */}
      <div
        className={`pointer-events-none absolute inset-x-0 top-0 z-20 flex justify-center transition duration-200 ${
          (toolbar || chat.unread > 0) && connected && frameSize.w ? "translate-y-0 opacity-100" : "pointer-events-none invisible -translate-y-3 opacity-0"
        }`}
      >
        <div ref={barRef} data-testid="toolbar" className="drag pointer-events-auto mt-2 flex items-center gap-1 glass rounded-full border border-white/10 p-1 shadow-float" data-tauri-drag-region>
          <div className="flex items-center gap-2 pl-2 pr-3" data-tauri-drag-region>
            <span className={`h-2 w-2 rounded-full ${connected ? latencyColor : "bg-warn"}`} />
            <span className="max-w-[180px] truncate text-caption font-normal">{name}</span>
            {secured && (
              <span title={fingerprint ? `End-to-end encrypted\nPeer fingerprint: ${fingerprint}` : "End-to-end encrypted"} className="text-white/70">
                <ShieldCheck size={14} />
              </span>
            )}
            {route && <span className="rounded-[5px] bg-white/10 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-white/70">{route}</span>}
            {viewOnly && <span className="rounded-[5px] bg-white/10 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-white/70">View only</span>}
          </div>
          <Sep />
          {info && info.displays.length > 1 && (
            <>
              {info.displays.map((_, i) => (
                <TB key={i} label={`Display ${i + 1}`} active={i === displayIdx} onClick={() => switchDisplay(i)}>
                  <span className="text-caption font-semibold">{i + 1}</span>
                </TB>
              ))}
              <Sep />
            </>
          )}
          <TB label={fit === "fit" ? "Original size" : "Fit to window"} onClick={() => setFit(fit === "fit" ? "original" : "fit")}>
            {fit === "fit" ? <Scan size={17} /> : <Expand size={17} />}
          </TB>
          <QualityMenu
            value={imageQuality}
            onChange={setQ}
            sharpen={sharpen}
            onSharpen={(m) => {
              setSharpenState(m);
              saveSharpen(m);
            }}
          />
          <TB label="Statistics" active={showStats} onClick={() => setShowStats(!showStats)}>
            <Activity size={17} />
          </TB>
          <Sep />
          <TB label={chat.unread ? `Chat (${chat.unread} unread)` : "Chat"} active={chat.open} onClick={() => setChat((c) => ({ ...c, open: !c.open, unread: 0 }))}>
            <MessageSquare size={17} />
            {chat.unread > 0 && (
              <span data-testid="chat-unread" className="absolute -right-0.5 -top-0.5 grid h-4 min-w-4 place-items-center rounded-full bg-danger px-1 text-[10px] font-semibold leading-none text-white">
                {chat.unread}
              </span>
            )}
          </TB>
          <TB label="Transfer files" onClick={() => act(api.openSession(peer, "file-transfer", key))}>
            <FolderSync size={17} />
          </TB>
          {recMode !== "off" && (
            <TB
              label={rec.state === "on" ? "Stop recording" : rec.state === "pending" ? "Waiting for the remote user to allow recording" : "Record session"}
              active={rec.state !== "idle"}
              onClick={toggleRecording}
            >
              <Video size={17} className={rec.state === "on" ? "text-danger" : rec.state === "pending" ? "animate-pulse" : ""} />
            </TB>
          )}
          {kbd && <KeyboardMenu kbd={kbd} mac={/mac/i.test(info?.platform ?? "")} onChange={setKeyboard} onForceQuit={() => act(invoke("session_mac_force_quit", { key }))} />}
          <ActionsMenu
            windows={(info?.platform ?? "").toLowerCase().startsWith("windows")}
            service={!!info?.sas_enabled}
            onCad={() => act(invoke("session_ctrl_alt_del", { key }))}
            onAsk={setAsk}
          />
          <Sep />
          <TB label={fullscreen ? "Exit full screen" : "Full screen"} onClick={toggleFullscreen}>
            {fullscreen ? <Minimize size={17} /> : <Maximize size={17} />}
          </TB>
          <TB label="Minimize" onClick={minimize}>
            <Minus size={17} />
          </TB>
          <TB label={pinned ? "Unpin toolbar" : "Pin toolbar"} onClick={() => setPinned(!pinned)}>
            {pinned ? <PinOff size={17} /> : <Pin size={17} />}
          </TB>
          <button
            aria-label="Disconnect"
            title="Disconnect"
            onClick={() => act(import("@tauri-apps/api/window").then((m) => m.getCurrentWindow().close()))}
            className="no-drag ml-1 grid h-9 w-9 place-items-center rounded-full bg-danger text-white transition hover:brightness-110"
          >
            <Power size={16} />
          </button>
        </div>
      </div>

      {ask && connected && (
        <ConfirmAction
          what={ask}
          windows={(info?.platform ?? "").toLowerCase().startsWith("windows")}
          service={!!info?.sas_enabled}
          onCancel={() => setAsk(null)}
          onConfirm={() => {
            setAsk(null);
            act(invoke(ask === "lock" ? "session_lock_screen" : "session_restart_remote", { key }));
          }}
        />
      )}

      {showStats && connected && (
        <div className="animate-fade pointer-events-none absolute bottom-3 left-3 z-10 rounded-xl border border-white/10 bg-black/70 px-3.5 py-2.5 font-mono text-footnote leading-[1.7] text-white/85 backdrop-blur">
          <Stat k="fps" v={vstats ? `${vstats.fps} shown / ${vstats.inFps} recv` : "–"} />
          <Stat k="latency" v={delay != null ? `${delay} ms` : "–"} />
          <Stat k="decode" v={vstats ? `${vstats.decodeMs} ms` : "–"} />
          <Stat k="render" v={vstats ? `${vstats.renderMs} ms · sharpen ${vstats.sharpen}` : "–"} />
          <Stat k="stall" v={vstats ? `${vstats.maxGapMs} ms max gap · ${vstats.keyRequests} key req · ${vstats.dropped} dropped` : "–"} />
          <Stat k="codec" v={vstats?.codec || quality.codec || "–"} />
          <Stat k="bitrate" v={quality.target_bitrate ? `${(quality.target_bitrate / 1000).toFixed(1)} Mbps` : "–"} />
          <Stat k="speed" v={quality.speed ?? "–"} />
          <Stat k="frame" v={vstats?.width ? `${vstats.width}×${vstats.height} (coded ${vstats.coded})` : "–"} />
          <Stat k="route" v={route ?? "–"} />
          <Stat k="encryption" v={secured == null ? "–" : secured ? "end-to-end" : "NONE"} />
          {fingerprint && <Stat k="fingerprint" v={fingerprint.slice(0, 23)} />}
        </div>
      )}

      {chat.open && (
        <ChatPanel
          msgs={chat.msgs}
          onClose={() => setChat((c) => ({ ...c, open: false }))}
          onSend={(text) => {
            act(invoke("session_chat", { key, text }));
            setChat((c) => ({ ...c, msgs: [...c.msgs, { me: true, text }] }));
          }}
        />
      )}
    </div>
  );
}

async function setWindowTitle(t: string) {
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTitle(t);
  } catch {
    /* not in tauri */
  }
}

function Sep() {
  return <div className="mx-1 h-5 w-px bg-white/10" />;
}

function TB({ label, active, children, onClick }: { label: string; active?: boolean; children: React.ReactNode; onClick?: () => void }) {
  return (
    <button
      aria-label={label}
      title={label}
      onClick={onClick}
      className={`no-drag relative grid h-9 min-w-9 place-items-center rounded-full px-2 transition ${active ? "bg-white/15 text-white" : "text-white/70 hover:bg-white/10 hover:text-white"}`}
    >
      {children}
    </button>
  );
}

function Stat({ k, v }: { k: string; v: React.ReactNode }) {
  return (
    <div className="flex justify-between gap-6">
      <span className="text-white/45">{k}</span>
      <span>{v}</span>
    </div>
  );
}

/** Largest size of a w×h picture inside the stage, keeping its aspect ratio. */
export function fitSize(f: { w: number; h: number }, stage: { w: number; h: number }) {
  // No picture yet: no size (a WebGL canvas sizes itself to its box, which must not follow it).
  if (!f.w || !f.h) return { width: 0, height: 0 };
  if (!stage.w || !stage.h) return { maxWidth: "100%", maxHeight: "100%" };
  const s = Math.min(stage.w / f.w, stage.h / f.h);
  return { width: Math.floor(f.w * s), height: Math.floor(f.h * s) };
}

type KbdSettings = { dir: "pc-mac" | "mac-pc" | ""; shortcuts: boolean; system_keys: boolean; alt_tab_remote: boolean; hook: boolean };

/** Keyboard: PC/Mac shortcut mapping, system keys (hook) and Alt+Tab. See docs/keyboard.md. */
function KeyboardMenu({ kbd, mac, onChange, onForceQuit }: { kbd: KbdSettings; mac: boolean; onChange: (v: Partial<Record<"shortcuts" | "systemKeys" | "altTabRemote", boolean>>) => void; onForceQuit: () => void }) {
  const [open, setOpen] = useState(false);
  const item = (label: string, hint: string, on: boolean | null, run: () => void) => (
    <button
      key={label}
      role={on == null ? "menuitem" : "menuitemcheckbox"}
      aria-checked={on ?? undefined}
      onClick={() => {
        setOpen(false);
        run();
      }}
      className="flex w-full items-start gap-2 rounded-lg px-3 py-2 text-left transition hover:bg-white/10"
    >
      <span className="mt-0.5 w-4 shrink-0">{on && <Check size={15} />}</span>
      <span className="flex flex-col">
        <span className="text-caption font-normal">{label}</span>
        <span className="text-footnote text-white/50">{hint}</span>
      </span>
    </button>
  );
  return (
    <div className="no-drag relative">
      <TB label="Keyboard" active={open} onClick={() => setOpen(!open)}>
        <Keyboard size={17} />
      </TB>
      {open && (
        <div role="menu" className="animate-fade absolute right-0 top-11 z-30 w-80 overflow-hidden rounded-xl border border-white/10 bg-[#272729] p-1 shadow-float" onMouseLeave={() => setOpen(false)}>
          {kbd.dir === "pc-mac" && item("Windows shortcuts on the Mac", "Ctrl works as ⌘ (Ctrl+C, Ctrl+V…), the Win key as Control, Home/End, Ctrl+arrows like on Windows. Off: keys as on a Mac keyboard (e.g. for a terminal).", kbd.shortcuts, () => onChange({ shortcuts: !kbd.shortcuts }))}
          {kbd.dir === "mac-pc" && item("Mac shortcuts on the PC", "⌘ works as Ctrl (⌘C, ⌘V…), Control as the Windows key, ⌘/⌥+arrows like on a Mac.", kbd.shortcuts, () => onChange({ shortcuts: !kbd.shortcuts }))}
          {kbd.hook && item("Send system keys", "Win, Win+R, Alt+Space, Alt+Esc, Ctrl+Esc, Print go to the remote computer while this window is active. Ctrl+Alt+Home gives them back to this PC until you click into the picture.", kbd.system_keys, () => onChange({ systemKeys: !kbd.system_keys }))}
          {kbd.hook && kbd.system_keys && item("Send Alt+Tab", "Off: Alt+Tab switches windows on this PC.", kbd.alt_tab_remote, () => onChange({ altTabRemote: !kbd.alt_tab_remote }))}
          {mac && item("Send ⌘⌥Esc", "Force Quit Applications on the Mac (instead of Ctrl+Alt+Del)", null, onForceQuit)}
        </div>
      )}
    </div>
  );
}

/** Ctrl+Alt+Del, lock and restart: disruptive, so grouped, explained and (lock, restart) confirmed. */
function ActionsMenu({ windows, service, onCad, onAsk }: { windows: boolean; service: boolean; onCad: () => void; onAsk: (a: "lock" | "restart") => void }) {
  const [open, setOpen] = useState(false);
  const item = (label: string, hint: string, run: () => void, disabled = false) => (
    <button
      key={label}
      disabled={disabled}
      onClick={() => {
        setOpen(false);
        run();
      }}
      className="flex w-full flex-col items-start rounded-lg px-3 py-2 text-left transition hover:bg-white/10 disabled:pointer-events-none disabled:opacity-45"
    >
      <span className="text-caption font-normal">{label}</span>
      <span className="text-footnote text-white/50">{hint}</span>
    </button>
  );
  return (
    <div className="no-drag relative">
      <TB label="Remote actions" active={open} onClick={() => setOpen(!open)}>
        <MonitorCog size={17} />
      </TB>
      {open && (
        <div role="menu" className="animate-fade absolute right-0 top-11 z-30 w-72 overflow-hidden rounded-xl border border-white/10 bg-[#272729] p-1 shadow-float" onMouseLeave={() => setOpen(false)}>
          {windows &&
            item(
              "Send Ctrl+Alt+Del",
              service ? "Opens the Windows security screen" : "Needs Tether installed on the remote PC. TetherQS cannot send it.",
              onCad,
              !service,
            )}
          {item("Lock screen…", service ? "Locks the remote computer" : "You will not be able to sign back in", () => onAsk("lock"))}
          {item("Restart…", "Ends the session", () => onAsk("restart"))}
        </div>
      )}
    </div>
  );
}

function ConfirmAction({ what, windows, service, onCancel, onConfirm }: { what: "lock" | "restart"; windows: boolean; service: boolean; onCancel: () => void; onConfirm: () => void }) {
  const lock = what === "lock";
  const title = lock ? "Lock the remote screen?" : "Restart the remote device?";
  const text = lock
    ? service
      ? "The remote computer locks. Its user signs back in; you see the lock screen and can type the password if you know it."
      : windows
        ? "Tether is not installed as a service on the remote PC (TetherQS): you will not see the lock screen and cannot sign back in. Only the person at the computer can unlock it."
        : "You may not be able to see the lock screen or sign back in remotely. The person at the computer may have to unlock it."
    : service
      ? "The session ends. Tether starts again with the computer; reconnect once it is back."
      : "The session ends. TetherQS does not start again by itself: the person at the computer has to start it and tell you the new password.";
  return (
    <div role="dialog" aria-label={title} className="animate-rise absolute left-1/2 top-16 z-30 w-[360px] -translate-x-1/2 rounded-2xl border border-white/10 glass p-4 text-center shadow-float">
      <div className="text-callout font-semibold">{title}</div>
      <div className={`mt-1 text-caption ${service ? "text-white/60" : "text-[#ffb340]"}`}>{text}</div>
      <div className="mt-4 flex justify-center gap-2">
        <Button variant="ghost" size="sm" className="!text-white/70 hover:!bg-white/10" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="primary" size="sm" onClick={onConfirm}>
          {lock ? (service ? "Lock" : "Lock anyway") : service ? "Restart" : "Restart anyway"}
        </Button>
      </div>
    </div>
  );
}

function QualityMenu({ value, onChange, sharpen, onSharpen }: { value: string; onChange: (v: string) => void; sharpen: SharpenMode; onSharpen: (m: SharpenMode) => void }) {
  const [open, setOpen] = useState(false);
  const cur = QUALITY.find((q) => q.value === value) ?? QUALITY[1];
  return (
    <div className="no-drag relative">
      <button
        title="Image quality"
        onClick={() => setOpen(!open)}
        className="flex h-9 items-center gap-1.5 rounded-full px-3 text-caption text-white/75 transition hover:bg-white/10 hover:text-white"
      >
        <Gauge size={16} />
        {cur.label}
        <ChevronDown size={14} />
      </button>
      {open && (
        <div className="animate-fade absolute left-0 top-11 w-56 overflow-hidden rounded-xl border border-white/10 bg-[#272729] p-1 shadow-float" onMouseLeave={() => setOpen(false)}>
          {QUALITY.map((q) => (
            <button
              key={q.value}
              onClick={() => {
                onChange(q.value);
                setOpen(false);
              }}
              className={`flex w-full flex-col items-start rounded-lg px-3 py-2 text-left transition hover:bg-white/10 ${q.value === value ? "bg-white/10" : ""}`}
            >
              <span className="text-caption font-normal">{q.label}</span>
              <span className="text-footnote text-white/50">{q.hint}</span>
            </button>
          ))}
          <div className="mx-3 mb-1 mt-2 border-t border-white/10 pt-2 text-footnote text-white/50">Sharpening</div>
          <div className="flex gap-1 px-2 pb-1" role="radiogroup" aria-label="Sharpening">
            {SHARPEN_MODES.map((m) => (
              <button
                key={m}
                role="radio"
                aria-checked={m === sharpen}
                title={SHARPEN[m].hint}
                onClick={() => onSharpen(m)}
                className={`flex-1 rounded-lg px-2 py-1.5 text-caption transition hover:bg-white/10 ${m === sharpen ? "bg-white/10 text-white" : "text-white/70"}`}
              >
                {SHARPEN[m].label}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function Overlay({ phase, name, peer, sessionKey: key, hostMissing, onLogin }: { phase: Phase; name: string; peer: string; sessionKey: string; hostMissing: string[]; onLogin: () => void }) {
  const [pw, setPw] = useState("");
  const [remember, setRemember] = useState(true);
  const close = async () => (await import("@tauri-apps/api/window")).getCurrentWindow().close();
  return (
    <div className="absolute inset-0 flex flex-col">
      <TitleBar className="text-white">
        <Logo size={20} />
        <span className="text-caption font-normal text-white/80">{name}</span>
      </TitleBar>
      <div className="grid flex-1 place-items-center p-6">
        <div className="animate-rise w-[380px] max-w-full text-center">
          <div className="mx-auto grid h-16 w-16 place-items-center rounded-2xl bg-white/5">
            <MonitorSmartphone size={28} className="text-white/80" />
          </div>
          <div className={`mt-4 text-headline font-semibold ${name === formatId(peer) ? "font-mono" : ""}`}>{name}</div>
          {name !== formatId(peer) && <div className="font-mono text-caption text-white/50">{formatId(peer)}</div>}

          {phase.kind === "connecting" && (
            <div className="mt-8 flex flex-col items-center gap-4">
              <Spinner />
              <div className="text-caption text-white/70">{phase.text}</div>
              <Button variant="ghost" size="sm" className="!text-white/60 hover:!bg-white/10" onClick={close}>
                Cancel
              </Button>
            </div>
          )}

          {phase.kind === "connected" && <FirstImage hostMissing={hostMissing} />}

          {phase.kind === "password" && (
            <form
              className="mt-7 text-left"
              onSubmit={(e) => {
                e.preventDefault();
                if (!pw) return;
                onLogin();
                invoke("session_login", { key, password: pw, remember });
                setPw("");
              }}
            >
              <label className="text-caption text-white/60" htmlFor="pw">
                {phase.wrong ? <span className="text-danger">Wrong password, try again</span> : "Enter the one-time password shown on the remote device"}
              </label>
              {phase.why && <p className="mt-1 text-footnote text-white/70">{phase.why}</p>}
              <p className="mt-1 text-footnote text-white/45">Or ask the person there to click Accept: that lets you in without a password, and this window continues by itself.</p>
              <input
                id="pw"
                type="password"
                autoFocus
                value={pw}
                onChange={(e) => setPw(e.target.value)}
                className={`mt-2 h-12 w-full rounded-xl border bg-white/5 px-4 text-callout text-white outline-none transition focus:border-accent ${phase.wrong ? "border-danger" : "border-white/10"}`}
              />
              <label className="mt-3 flex items-center gap-2 text-caption text-white/60">
                <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} className="accent-[var(--accent)]" />
                Remember password for this device
              </label>
              <div className="mt-5 flex gap-2">
                <Button type="button" variant="ghost" className="flex-1 !text-white/70 hover:!bg-white/10" onClick={close}>
                  Cancel
                </Button>
                <Button type="submit" variant="primary" className="flex-1" disabled={!pw}>
                  Connect
                </Button>
              </div>
            </form>
          )}

          {phase.kind === "error" && (
            <div className="mt-7">
              <div className="text-callout font-normal text-danger">{phase.title}</div>
              <div className="mt-1.5 text-caption leading-relaxed text-white/65">{phase.text}</div>
              <div className="mt-6 flex justify-center gap-2">
                <Button variant="ghost" className="!text-white/70 hover:!bg-white/10" onClick={close}>
                  Close
                </Button>
                {phase.retry !== false && (
                  <Button variant="primary" onClick={() => invoke("session_reconnect", { key, forceRelay: false })}>
                    Try again
                  </Button>
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Connected, no picture yet. A Mac without Screen Recording permission never sends one. */
function FirstImage({ hostMissing }: { hostMissing: string[] }) {
  const [late, setLate] = useState(false);
  useEffect(() => {
    const t = setTimeout(() => setLate(true), 8000);
    return () => clearTimeout(t);
  }, []);
  const screen = hostMissing.includes("screen-recording");
  return (
    <div className="mt-8 flex flex-col items-center gap-4">
      <Spinner />
      <div className="text-caption text-white/70">{screen ? "The Mac does not share its screen yet" : "Waiting for the first image…"}</div>
      {(screen || late) && (
        <div role="status" className="max-w-[420px] rounded-xl border border-white/10 bg-white/5 px-4 py-3 text-left text-footnote leading-relaxed text-white/70">
          {screen ? "The Mac has not allowed Tether to record its screen. " : "Still no picture. If the remote computer is a Mac, it probably has not allowed Tether to record its screen. "}
          Ask the person there to open <b className="font-semibold text-white/85">System Settings → Privacy &amp; Security → Screen &amp; System Audio Recording</b> and switch Tether on
          {hostMissing.includes("accessibility") ? <> (and Tether under <b className="font-semibold text-white/85">Accessibility</b>, for mouse and keyboard)</> : null}. If it is already on, it belongs to an older version: the warning in the Mac app has a Fix button that clears it. Then quit and reopen the app there and connect again.
        </div>
      )}
    </div>
  );
}

function Spinner() {
  return <div className="h-7 w-7 animate-spin rounded-full border-2 border-white/15 border-t-[#2997ff]" />;
}

/** Over the dimmed last frame while the session reconnects on its own. */
function ReconnectOverlay({ attempt, text }: { attempt: number; text: string }) {
  const close = async () => (await import("@tauri-apps/api/window")).getCurrentWindow().close();
  return (
    <div className="absolute inset-0 grid place-items-center p-6">
      <div role="status" className="animate-rise flex w-[320px] max-w-full flex-col items-center gap-3 rounded-2xl border border-white/10 glass p-6 text-center shadow-float">
        <Spinner />
        <div className="text-callout font-semibold">Reconnecting… (attempt {attempt})</div>
        {text && <div className="text-caption text-white/60">{text}</div>}
        <Button variant="ghost" size="sm" className="!text-white/60 hover:!bg-white/10" onClick={close}>
          Disconnect
        </Button>
      </div>
    </div>
  );
}

function RecPill({ since }: { since: number }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const s = Math.max(0, Math.floor((Date.now() - since) / 1000));
  const mmss = `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
  return (
    <div className="pointer-events-none absolute left-3 top-3 z-10 flex items-center gap-2 rounded-full border border-white/10 glass px-3 py-1 font-mono text-footnote">
      <span className="h-2 w-2 animate-pulse rounded-full bg-danger" />
      <span className="font-semibold text-danger">REC</span>
      <span className="text-white/80">{mmss}</span>
    </div>
  );
}

function ChatPanel({ msgs, onSend, onClose }: { msgs: { me: boolean; text: string }[]; onSend: (t: string) => void; onClose: () => void }) {
  const [v, setV] = useState("");
  const end = useRef<HTMLDivElement>(null);
  // Braces: scrollIntoView returns a Promise in current WebView2, which React would call as cleanup.
  useEffect(() => {
    end.current?.scrollIntoView({ behavior: "smooth" });
  }, [msgs.length]);
  return (
    <div className="animate-rise absolute bottom-3 right-3 z-20 flex h-[380px] w-[320px] flex-col overflow-hidden rounded-2xl border border-white/10 glass shadow-float">
      <div className="flex items-center justify-between border-b border-white/10 px-4 py-2.5">
        <span className="text-caption font-normal">Chat</span>
        <IconButton label="Close chat" onClick={onClose} className="!text-white/60 hover:!bg-white/10">
          <X size={16} />
        </IconButton>
      </div>
      <div className="flex-1 space-y-2 overflow-y-auto p-3">
        {msgs.length === 0 && <div className="pt-10 text-center text-caption text-white/40">Messages appear on the remote screen.</div>}
        {msgs.map((m, i) => (
          <div key={i} className={`flex ${m.me ? "justify-end" : ""}`}>
            <div className={`max-w-[80%] rounded-2xl px-3 py-1.5 text-caption ${m.me ? "bg-accent text-white" : "bg-white/10"}`}>{m.text}</div>
          </div>
        ))}
        <div ref={end} />
      </div>
      <form
        className="flex gap-2 border-t border-white/10 p-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (v.trim()) onSend(v.trim());
          setV("");
        }}
      >
        <input value={v} onChange={(e) => setV(e.target.value)} placeholder="Message" className="h-9 flex-1 rounded-xl bg-white/5 px-3 text-caption text-white outline-none placeholder:text-white/35" />
        <button aria-label="Send" className="grid h-9 w-9 place-items-center rounded-full bg-accent text-white">
          <Send size={15} />
        </button>
      </form>
    </div>
  );
}
