import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  ChevronDown,
  Expand,
  FolderSync,
  Gauge,
  Keyboard,
  Lock,
  Maximize,
  MessageSquare,
  Minimize,
  MonitorSmartphone,
  Pin,
  PinOff,
  Power,
  RotateCcw,
  Scan,
  Send,
  ShieldCheck,
  Video,
  X,
} from "lucide-react";
import { api, formatId, invoke, listen, videoChannel } from "../lib/ipc";
import { probeCodecs, VideoPipeline, type VideoStats } from "../lib/video";
import { Button, IconButton, Logo, TitleBar, useToast } from "../components/ui";
import { FileTransfer } from "./FileTransfer";
import { mouseMask, MOUSE, wheelSteps, lockModes } from "../lib/input";
import { classify, onFailure, reconnectMinutes, type Series } from "../lib/reconnect";

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
  | { kind: "password"; wrong: boolean }
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

export function Session({ sessionKey, peer, conn }: { sessionKey: string; peer: string; conn: string }) {
  if (conn === "file-transfer") return <FileTransfer sessionKey={sessionKey} peer={peer} />;
  return <RemoteView sessionKey={sessionKey} peer={peer} />;
}

function RemoteView({ sessionKey: key, peer }: { sessionKey: string; peer: string }) {
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
  const [direct, setDirect] = useState<boolean | null>(null);
  const [secured, setSecured] = useState<boolean | null>(null);
  const [fingerprint, setFingerprint] = useState("");
  const [showStats, setShowStats] = useState(false);
  const [toolbar, setToolbar] = useState(true);
  const [pinned, setPinned] = useState(false);
  const [chat, setChat] = useState<{ open: boolean; msgs: { me: boolean; text: string }[] }>({ open: false, msgs: [] });
  const [cursor, setCursor] = useState("default");
  const [imageQuality, setImageQuality] = useState("balanced");
  const [fullscreen, setFullscreen] = useState(false);
  const cursors = useRef(new Map<string, string>());
  const displayRef = useRef(display);
  displayRef.current = display;
  const displayIdxRef = useRef(displayIdx);
  displayIdxRef.current = displayIdx;
  // Reconnect: the running series, its timer and the policy limit (event handlers read refs).
  const series = useRef<Series | null>(null);
  const retryTimer = useRef(0);
  const limitMin = useRef(10);
  const everConnected = useRef(false);
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

  // ---------------------------------------------------------------- connection + events
  useEffect(() => {
    let alive = true;
    const unsubs: (() => void)[] = [];
    (async () => {
      const canvas = canvasRef.current!;
      const pipe = new VideoPipeline(canvas, {
        onFail: (codec, d) => invoke("session_webcodecs_failed", { key, codec, display: d }),
        onNeedKey: (d) => invoke("session_refresh", { key, display: d }),
        onSize: (w, h) => setFrameSize({ w, h }),
      });
      pipeRef.current = pipe;
      const formats = await probeCodecs();
      limitMin.current = reconnectMinutes(await invoke<string>("session_policy_get", { id: "connect.reconnect-minutes" }).catch(() => ""));
      const mode = (await invoke<string>("session_policy_get", { id: "recording.mode" }).catch(() => "")) || "off";
      recModeRef.current = mode;
      setRecMode(mode);
      // Uploads left over from a closed window or an earlier run.
      if (mode !== "off") invoke("recording_finished").catch(() => {});
      unsubs.push(
        await listen("session", (e) => {
          const p = e.payload;
          switch (e.name) {
            case "msgbox":
              onMsgbox(p.type, p.title, p.text, p.retry);
              break;
            case "peer_info":
              setInfo(p);
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
              pipe.setDisplay(p.display);
              break;
            case "close_success":
              setPhase({ kind: "connected" });
              break;
            case "connection_type":
              setDirect(p.direct);
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
              setChat((c) => ({ open: true, msgs: [...c.msgs, { me: false, text: p.text }] }));
              break;
          }
        }),
      );
      if (!alive) return;
      const ch = videoChannel((buf) => pipe.push(buf));
      await invoke("session_start", { key, peer, webcodecs: formats, video: ch });
      setImageQuality((await invoke<string>("get_option", { key: "image-quality" }).catch(() => "")) || "balanced");
    })();
    const t = setInterval(() => pipeRef.current && setVstats({ ...pipeRef.current.stats }), 1000);
    return () => {
      alive = false;
      clearInterval(t);
      clearTimeout(retryTimer.current);
      if (recRef.current.state === "on") invoke("recording_finished").catch(() => {});
      unsubs.forEach((u) => u());
      pipeRef.current?.close();
      invoke("session_close", { key });
    };
  }, [key, peer]);

  const onMsgbox = (type: string, title: string, text: string, retry: boolean) => {
    if (type === "connecting" && series.current) return; // keep the reconnect overlay
    if (type === "input-password" || type === "re-input-password") {
      series.current = null;
      clearTimeout(retryTimer.current);
    }
    if (type === "input-password") return setPhase({ kind: "password", wrong: false });
    if (type === "re-input-password") return setPhase({ kind: "password", wrong: true });
    if (type === "connecting") return setPhase({ kind: "connecting", text: friendly(text) || "Connecting…" });
    if (type === "success" || type === "info" || type === "nocancel-info") return;
    if (text.toLowerCase().includes("waiting") || type.includes("wait")) return setPhase({ kind: "connecting", text: "Waiting for the remote user to accept…" });
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
    invoke("session_record", { key, start: false });
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
    const send = (mask: number, e: MouseEvent, xy = toRemote(e)) => invoke("session_mouse", { key, mask, ...xy, ...mods(e) });

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
    const down = (e: PointerEvent) => {
      c.setPointerCapture(e.pointerId);
      c.focus();
      send(mouseMask(MOUSE.DOWN, e.button), e);
    };
    const up = (e: PointerEvent) => send(mouseMask(MOUSE.UP, e.button), e);
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      const s = wheelSteps(e, wheelAcc);
      wheelAcc = s.acc;
      if (s.x || s.y) invoke("session_mouse", { key, mask: MOUSE.WHEEL, x: s.x, y: s.y, ...mods(e) });
    };
    const ctx = (e: MouseEvent) => e.preventDefault();
    const key_ = (down: boolean) => (e: KeyboardEvent) => {
      if (chat.open && (e.target as HTMLElement)?.tagName === "INPUT") return;
      e.preventDefault();
      invoke("session_key", { key, code: e.code, down, lockModes: lockModes(e), character: e.key.length === 1 ? e.key : "" });
    };
    const kd = key_(true);
    const ku = key_(false);
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
    };
  }, [phase.kind, key, toRemote, chat.open]);

  // Auto-hide the toolbar unless pinned; reveal at the top edge.
  useEffect(() => {
    if (pinned || phase.kind !== "connected") return setToolbar(true);
    let t = window.setTimeout(() => setToolbar(false), 2200);
    const m = (e: MouseEvent) => {
      if (e.clientY < 56) {
        setToolbar(true);
        clearTimeout(t);
        t = window.setTimeout(() => setToolbar(false), 2200);
      }
    };
    window.addEventListener("mousemove", m);
    return () => {
      clearTimeout(t);
      window.removeEventListener("mousemove", m);
    };
  }, [pinned, phase.kind]);

  const toggleFullscreen = async () => {
    const { getCurrentWindow } = await import("@tauri-apps/api/window").catch(() => ({ getCurrentWindow: null as any }));
    if (getCurrentWindow) {
      const w = getCurrentWindow();
      const f = !(await w.isFullscreen());
      await w.setFullscreen(f);
      setFullscreen(f);
    }
  };

  const switchDisplay = (i: number) => {
    invoke("session_switch_display", { key, display: i });
    pipeRef.current?.setDisplay(i);
    setDisplayIdx(i);
    if (info?.displays[i]) setDisplay(info.displays[i]);
  };

  const setQ = (v: string) => {
    setImageQuality(v);
    invoke("session_quality", { key, quality: v });
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
          ref={canvasRef}
          tabIndex={0}
          style={{
            cursor,
            ...(fit === "fit"
              ? { maxWidth: "100%", maxHeight: "100%", width: "auto", height: "auto", objectFit: "contain" }
              : { width: frameSize.w / devicePixelRatio, height: frameSize.h / devicePixelRatio }),
            opacity: connected && frameSize.w ? 1 : phase.kind === "reconnecting" ? 0.35 : 0,
          }}
          className="block outline-none transition-opacity duration-300"
        />
        {phase.kind === "reconnecting" ? (
          <ReconnectOverlay attempt={phase.attempt} text={phase.text} />
        ) : phase.kind !== "connected" || !frameSize.w ? (
          <Overlay phase={phase} name={name} peer={peer} sessionKey={key} />
        ) : null}
        {rec.state === "on" && connected && <RecPill since={rec.since} />}
        {toast.node}
      </div>

      {/* floating toolbar */}
      <div
        className={`pointer-events-none absolute inset-x-0 top-0 z-20 flex justify-center transition duration-200 ${
          toolbar && connected && frameSize.w ? "translate-y-0 opacity-100" : "pointer-events-none invisible -translate-y-3 opacity-0"
        }`}
      >
        <div className="drag pointer-events-auto mt-2 flex items-center gap-1 glass rounded-full border border-white/10 p-1 shadow-float" data-tauri-drag-region>
          <div className="flex items-center gap-2 pl-2 pr-3" data-tauri-drag-region>
            <span className={`h-2 w-2 rounded-full ${connected ? latencyColor : "bg-warn"}`} />
            <span className="max-w-[180px] truncate text-caption font-normal">{name}</span>
            {secured && (
              <span title={fingerprint ? `End-to-end encrypted\nPeer fingerprint: ${fingerprint}` : "End-to-end encrypted"} className="text-white/70">
                <ShieldCheck size={14} />
              </span>
            )}
            {direct != null && <span className="rounded-[5px] bg-white/10 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-white/70">{direct ? "P2P" : "Relay"}</span>}
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
          <QualityMenu value={imageQuality} onChange={setQ} />
          <TB label="Statistics" active={showStats} onClick={() => setShowStats(!showStats)}>
            <Activity size={17} />
          </TB>
          <Sep />
          <TB label="Send Ctrl+Alt+Del" onClick={() => invoke("session_ctrl_alt_del", { key })}>
            <Keyboard size={17} />
          </TB>
          <TB label="Lock remote screen" onClick={() => invoke("session_lock_screen", { key })}>
            <Lock size={17} />
          </TB>
          <TB label="Chat" active={chat.open} onClick={() => setChat((c) => ({ ...c, open: !c.open }))}>
            <MessageSquare size={17} />
          </TB>
          <TB label="Transfer files" onClick={() => api.openSession(peer, "file-transfer")}>
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
          <TB label="Restart remote device" onClick={() => confirm("Restart the remote device?") && invoke("session_restart_remote", { key })}>
            <RotateCcw size={17} />
          </TB>
          <Sep />
          <TB label={fullscreen ? "Exit full screen" : "Full screen"} onClick={toggleFullscreen}>
            {fullscreen ? <Minimize size={17} /> : <Maximize size={17} />}
          </TB>
          <TB label={pinned ? "Unpin toolbar" : "Pin toolbar"} onClick={() => setPinned(!pinned)}>
            {pinned ? <PinOff size={17} /> : <Pin size={17} />}
          </TB>
          <button
            aria-label="Disconnect"
            title="Disconnect"
            onClick={async () => (await import("@tauri-apps/api/window")).getCurrentWindow().close()}
            className="no-drag ml-1 grid h-9 w-9 place-items-center rounded-full bg-danger text-white transition hover:brightness-110"
          >
            <Power size={16} />
          </button>
        </div>
      </div>

      {showStats && connected && (
        <div className="animate-fade pointer-events-none absolute bottom-3 left-3 z-10 rounded-xl border border-white/10 bg-black/70 px-3.5 py-2.5 font-mono text-footnote leading-[1.7] text-white/85 backdrop-blur">
          <Stat k="fps" v={vstats ? `${vstats.fps} shown / ${vstats.inFps} recv` : "–"} />
          <Stat k="latency" v={delay != null ? `${delay} ms` : "–"} />
          <Stat k="decode" v={vstats ? `${vstats.decodeMs} ms` : "–"} />
          <Stat k="codec" v={vstats?.codec || quality.codec || "–"} />
          <Stat k="bitrate" v={quality.target_bitrate ? `${(quality.target_bitrate / 1000).toFixed(1)} Mbps` : "–"} />
          <Stat k="speed" v={quality.speed ?? "–"} />
          <Stat k="frame" v={vstats?.width ? `${vstats.width}×${vstats.height} (coded ${vstats.coded})` : "–"} />
          <Stat k="route" v={direct == null ? "–" : direct ? "direct" : "relay"} />
          <Stat k="encryption" v={secured == null ? "–" : secured ? "end-to-end" : "NONE"} />
          {fingerprint && <Stat k="fingerprint" v={fingerprint.slice(0, 23)} />}
        </div>
      )}

      {chat.open && (
        <ChatPanel
          msgs={chat.msgs}
          onClose={() => setChat((c) => ({ ...c, open: false }))}
          onSend={(text) => {
            invoke("session_chat", { key, text });
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
      className={`no-drag grid h-9 min-w-9 place-items-center rounded-full px-2 transition ${active ? "bg-white/15 text-white" : "text-white/70 hover:bg-white/10 hover:text-white"}`}
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

function QualityMenu({ value, onChange }: { value: string; onChange: (v: string) => void }) {
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
        </div>
      )}
    </div>
  );
}

function Overlay({ phase, name, peer, sessionKey: key }: { phase: Phase; name: string; peer: string; sessionKey: string }) {
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

          {phase.kind === "connected" && (
            <div className="mt-8 flex flex-col items-center gap-4">
              <Spinner />
              <div className="text-caption text-white/70">Waiting for the first image…</div>
            </div>
          )}

          {phase.kind === "password" && (
            <form
              className="mt-7 text-left"
              onSubmit={(e) => {
                e.preventDefault();
                if (!pw) return;
                invoke("session_login", { key, password: pw, remember });
                setPw("");
              }}
            >
              <label className="text-caption text-white/60" htmlFor="pw">
                {phase.wrong ? <span className="text-danger">Wrong password, try again</span> : "Enter the password shown on the remote device"}
              </label>
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
  useEffect(() => end.current?.scrollIntoView({ behavior: "smooth" }), [msgs.length]);
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
