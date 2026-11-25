// TetherQS: one 420×560 window. Visual rules: /DESIGN.md.
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Copy, Eye, EyeOff, FolderSync, HelpCircle, Keyboard, Minus, RefreshCw, Send, ShieldAlert, ShieldCheck, X } from "lucide-react";
import { onAccent } from "./color";
import { type AgentConsent, api, type Client, type Diagnostics, formatId, type Probe, inTauri, onAgentConsent, onCm, type State } from "./ipc";

type Consent = { conn_id: number; peer_name: string; text: string };
type Msg = { me: boolean; text: string };

async function win() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export function App() {
  const [s, setS] = useState<State | null>(null);
  const [help, setHelp] = useState(false);
  const [consent, setConsent] = useState<Consent | null>(null);
  const [agentAsk, setAgentAsk] = useState<AgentConsent | null>(null);
  const [rec, setRec] = useState<Set<number>>(new Set());
  const [toast, setToast] = useState<string | null>(null);
  const [chat, setChat] = useState<Record<number, Msg[]>>({});
  const addMsg = (id: number, m: Msg) => setChat((c) => ({ ...c, [id]: [...(c[id] ?? []), m] }));
  const since = useRef(new Map<number, number>());

  const refresh = () => api.state().then(setS).catch(() => {});
  const toastTimer = useRef(0);
  const toastEl = useRef<HTMLDivElement>(null);
  const say = (m: string) => {
    setToast(m);
    // An earlier message's timer must not hide this one.
    clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 2500);
  };

  useEffect(() => {
    if (!inTauri) return;
    refresh();
    const t = setInterval(refresh, 1000);
    const un = onCm((name, p) => {
      if (name === "record_consent_request") setConsent(p);
      else if (name === "chat") {
        addMsg(p.id, { me: false, text: p.text });
        say("New message from your technician");
      }
      else if (name === "refused") say(p.reason === "update-required" ? "A connection was refused: update TetherQS first." : "Incoming sessions are turned off by your administrator.");
      refresh();
    });
    // The health agent's pre-dialog: on the event, and once at start (a reload while it waits).
    const ask = () => api.agentConsent().then(setAgentAsk, () => {});
    ask();
    const unAsk = onAgentConsent(ask);
    return () => {
      clearInterval(t);
      un.then((f) => f());
      unAsk.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!s) return;
    const r = document.documentElement.style;
    // The brand colour can be anything: its text is black or white, whichever reads better, in
    // both themes. Not a colour: the theme's own accent.
    const on = onAccent(s.branding.accent);
    for (const [k, v] of [["--accent", s.branding.accent], ["--focus", s.branding.accent], ["--on-accent", on]] as const) {
      if (on) r.setProperty(k, v);
      else r.removeProperty(k);
    }
    document.title = s.branding.title;
  }, [s?.branding.accent, s?.branding.title]);

  // A request or a session start must be seen (Accept, Disconnect): leave the Help view.
  const sessions = s?.clients.map((c) => `${c.id}:${c.authorized}`).join() ?? "";
  useEffect(() => {
    if (sessions) setHelp(false);
  }, [sessions]);

  // Into the top layer, above the consent dialog; again when the dialog opens after the toast
  // (its own effect ran first). Without popover support (older macOS): a plain fixed element.
  useEffect(() => {
    const el = toastEl.current;
    if (!el?.showPopover) return;
    if (el.matches(":popover-open")) el.hidePopover();
    el.showPopover();
  }, [toast, consent]);

  if (!s) return <div className="h-full bg-canvas" />;

  // File transfer opened from an accepted session joins that session's card (no second entry).
  const companionOf = (c: Client) => (c.is_file_transfer ? s.clients.find((m) => m.authorized && !m.is_file_transfer && m.peer_id === c.peer_id) : undefined);
  const companions = (m: Client) => s.clients.filter((c) => c.authorized && companionOf(c) === m);
  const pending = s.clients.find((c) => !c.authorized);
  const active = s.clients.filter((c) => c.authorized && !companionOf(c));
  for (const c of active) if (!since.current.has(c.id)) since.current.set(c.id, Date.now());

  return (
    <div className="flex h-full flex-col">
      <TitleBar title={s.branding.title} logo={s.branding.logo} onHelp={s.branding.diagnostics ? () => setHelp(!help) : undefined} helpOpen={help} />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-6 pb-5">
        {help ? (
          <Help customApp={s.branding.custom_app} upload={s.branding.diagnostics_upload} status={s.status} lastCheck={s.update.last_check} stale={s.update.stale} onBack={() => setHelp(false)} />
        ) : (
          <>
            <UpdateBanner s={s} onError={say} />
            <MacBanner s={s} onError={say} />
            {/* Above the sessions, never instead of them: their Disconnect stays reachable. */}
            {pending && <Request key={pending.id} c={pending} s={s} onError={say} />}
            {active.length > 0 && (
              // Several cards: they keep their full height and the list scrolls (squeezed into the
              // window, each card's permission list shrank to nothing); below a request the whole
              // view scrolls.
              <div className={`-mx-2 flex flex-col px-2 ${pending ? "shrink-0" : "min-h-0 flex-1 overflow-y-auto"}`}>
              {active.map((c) => (
                <InSession
                  key={c.id}
                  stacked={active.length > 1 || !!pending}
                  c={c}
                  files={companions(c)}
                  s={s}
                  since={since.current.get(c.id)!}
                  rec={rec.has(c.id)}
                  onError={say}
                  msgs={chat[c.id] ?? []}
                  onSend={(text) =>
                    api.chat(c.id, text).then(
                      () => addMsg(c.id, { me: true, text }),
                      (e) => say(String(e)),
                    )
                  }
                />
              ))}
              </div>
            )}
            {!pending && !active.length && <Idle s={s} onCopied={say} />}
            <Footer s={s} onError={say} />
          </>
        )}
      </div>
      {consent && (
        <ConsentDialog
          c={consent}
          fallback={s.branding.consent_text}
          onAnswer={async (allow) => {
            setConsent(null);
            try {
              await api.recordConsent(consent.conn_id, allow);
              if (allow) setRec((r) => new Set(r).add(consent.conn_id));
            } catch (e) {
              say(String(e));
            }
          }}
        />
      )}
      {agentAsk && !consent && (
        <AgentConsentDialog
          key={agentAsk.id}
          d={agentAsk}
          onAnswer={(yes) => {
            setAgentAsk(null);
            api.agentConsentAnswer(agentAsk.id, yes).then(
              (ok) => !ok && say("That question is no longer open."),
              (e) => say(String(e)),
            );
          }}
        />
      )}
      {toast && (
        <div ref={toastEl} popover="manual" role="status" className="pointer-events-none fixed inset-x-0 bottom-6 top-auto z-50 flex w-full justify-center overflow-visible bg-transparent px-6">
          <div className="animate-rise rounded-full bg-ink px-5 py-2.5 text-center text-caption text-canvas shadow-float">{toast}</div>
        </div>
      )}
    </div>
  );
}

function TitleBar({ title, logo, onHelp, helpOpen }: { title: string; logo: string; onHelp?: () => void; helpOpen: boolean }) {
  const base = "no-drag grid h-8 w-9 place-items-center rounded-md hover:bg-parchment";
  const btn = `${base} text-muted hover:text-ink`;
  return (
    <div data-tauri-drag-region className="drag flex h-11 shrink-0 items-center gap-2 pl-6 pr-2">
      {logo && <img data-tauri-drag-region data-testid="qs-logo" src={logo} alt="" className="h-5 max-w-20 shrink-0 object-contain" draggable={false} />}
      <span data-tauri-drag-region data-testid="qs-title" className="min-w-0 flex-1 truncate text-caption font-semibold" title={title}>
        {title}
      </span>
      {onHelp && (
        <button aria-label="Help" title="Help" className={helpOpen ? `${base} text-accent` : btn} onClick={onHelp}>
          <HelpCircle size={16} />
        </button>
      )}
      <button aria-label="Minimize" className={btn} onClick={async () => (await win()).minimize()}>
        <Minus size={15} />
      </button>
      <button aria-label="Close" className={`${btn} hover:!bg-danger hover:!text-white dark:hover:!text-black`} onClick={async () => (await win()).close()}>
        <X size={16} />
      </button>
    </div>
  );
}

function Button({ variant = "pearl", className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "pearl" | "secondary" | "disconnect" }) {
  const v = {
    primary: "rounded-full bg-accent text-on-accent hover:brightness-110",
    secondary: "rounded-full border border-accent text-accent hover:bg-accent/10",
    pearl: "rounded-xl border border-hairline bg-tile-2 text-ink-80 hover:bg-parchment",
    // DESIGN.md: the in-session disconnect is the one red control. Not on-accent (that follows
    // the brand colour): white on the light theme's red, black on the dark theme's.
    disconnect: "rounded-full bg-danger text-white dark:text-black hover:brightness-110",
  }[variant];
  return <button className={`inline-flex h-11 select-none items-center justify-center gap-2 px-[22px] text-callout disabled:pointer-events-none disabled:opacity-40 ${v} ${className}`} {...rest} />;
}

function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-10 shrink-0 rounded-full disabled:opacity-40 ${checked ? "bg-accent" : "bg-faint"}`}
    >
      <span className={`absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white transition-transform duration-150 ${checked ? "translate-x-4" : ""}`} />
    </button>
  );
}

function Dot({ tone }: { tone: "ok" | "warn" | "danger" }) {
  return <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${{ ok: "bg-ok", warn: "bg-warn", danger: "bg-danger" }[tone]}`} />;
}

function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`animate-rise rounded-2xl border border-hairline bg-tile p-6 ${className}`}>{children}</div>;
}

function CopyButton({ text, label, onCopied }: { text: string; label: string; onCopied: (m: string) => void }) {
  const [done, setDone] = useState(false);
  return (
    <button
      aria-label={`Copy ${label}`}
      title={`Copy ${label}`}
      className="grid h-9 w-9 shrink-0 place-items-center rounded-full text-muted hover:bg-parchment hover:text-ink"
      onClick={async () => {
        await navigator.clipboard.writeText(text).catch(() => {});
        setDone(true);
        onCopied(`${label} copied`);
        setTimeout(() => setDone(false), 1500);
      }}
    >
      {done ? <Check size={16} className="text-ok" /> : <Copy size={16} />}
    </button>
  );
}

function Idle({ s, onCopied }: { s: State; onCopied: (m: string) => void }) {
  const clickOnly = s.branding.approve_mode === "click";
  // Hidden by default like the technician app (shoulder-surfing, screen sharing).
  const [reveal, setReveal] = useState(false);
  const icon = "grid h-9 w-9 shrink-0 place-items-center rounded-full text-muted hover:bg-parchment hover:text-ink";
  return (
    <>
      <p className="mb-5 mt-2 text-callout text-muted">{s.branding.welcome}</p>
      <Card>
        <div className="text-footnote text-muted">Your ID</div>
        <div className="mt-1 flex items-center gap-2">
          <span data-testid="qs-id" className="flex-1 select-text font-mono text-title font-semibold">
            {s.id ? formatId(s.id) : "—"}
          </span>
          {s.id && <CopyButton text={s.id} label="ID" onCopied={onCopied} />}
        </div>
        {!clickOnly && (
          <>
            <div className="my-5 h-px bg-hairline" />
            <div className="text-footnote text-muted">One-time password</div>
            <div className="mt-1 flex items-center gap-1">
              <span
                data-testid="qs-password"
                data-password={s.password}
                aria-label="One-time password"
                className={`flex-1 font-mono text-headline font-semibold tabular-nums ${reveal ? "select-text" : "select-none tracking-[0.12em]"}`}
              >
                {s.password ? (reveal ? s.password : "•".repeat(s.password.length)) : "—"}
              </span>
              <button aria-label={reveal ? "Hide password" : "Show password"} title={reveal ? "Hide password" : "Show password"} aria-pressed={reveal} className={icon} onClick={() => setReveal(!reveal)}>
                {reveal ? <EyeOff size={16} /> : <Eye size={16} />}
              </button>
              <CopyButton text={s.password} label="Password" onCopied={onCopied} />
              <button aria-label="New password" title="New password" className={icon} onClick={() => api.refreshPassword()}>
                <RefreshCw size={15} />
              </button>
            </div>
          </>
        )}
      </Card>
      {clickOnly && <p className="mt-4 text-caption text-muted">When your technician connects, you will be asked to accept here.</p>}
    </>
  );
}

function Avatar({ c }: { c: Client }) {
  const initials = (c.name || c.peer_id).replace(/[^A-Za-z0-9]/g, "").slice(0, 2).toUpperCase();
  return <div className="grid h-11 w-11 shrink-0 place-items-center rounded-full bg-accent text-callout font-semibold text-on-accent">{initials}</div>;
}

function Perms({ c, s, onError, also = [] }: { c: Client; s: State; onError: (m: string) => void; also?: Client[] }) {
  return (
    <div className="mt-3 space-y-0.5">
      {s.branding.perms.map((p) => {
        // What the connection has, not what policy defaults to. Locked off by policy: no switching
        // on, but one that is on all the same can still be switched off.
        const on = !!c[p.name];
        const off = p.locked && !p.allowed && !on;
        return (
        <div key={p.name} className="flex h-9 items-center gap-3 rounded-md px-1">
          <span className={`flex-1 text-caption ${off ? "text-faint" : ""}`}>{p.label}</span>
          <Toggle
            label={p.label}
            checked={on}
            disabled={off}
            onChange={(v) => {
              [c, ...also].forEach((x) => api.permission(x.id, p.name, v).catch((e) => onError(String(e))));
              // A running file transfer does not check the switch: Files off ends it.
              if (p.name === "file" && !v) also.forEach((x) => api.close(x.id));
            }}
          />
        </div>
        );
      })}
    </div>
  );
}

function Request({ c, s, onError }: { c: Client; s: State; onError: (m: string) => void }) {
  const what = c.is_file_transfer
    ? "transfer files with this computer"
    : c.is_view_camera
      ? "see this computer's camera"
      : c.is_terminal
        ? "open a command line on this computer"
        : c.port_forward
          ? `reach the network through this computer (${c.port_forward})`
          : "see and control this screen";
  return (
    // Not min-h-0: the card never shrinks below the permission list's minimum; the column scrolls.
    <Card className="mt-2 flex flex-1 flex-col !p-5">
      <div className="flex items-center gap-3">
        <Avatar c={c} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold" title={c.name || undefined}>
            {c.name || "Unknown"}
          </div>
          <div className="font-mono text-footnote text-muted">ID {formatId(c.peer_id)}</div>
        </div>
      </div>
      <div data-testid="qs-request-text" className="mt-3 flex items-start gap-2 rounded-xl border border-hairline bg-parchment px-3 py-2.5 text-caption">
        <ShieldAlert size={16} className="mt-0.5 shrink-0 text-warn" />
        <span>
          <b className="font-semibold">{c.name || "Someone"}</b> (ID {formatId(c.peer_id)}) wants to {what}. The name is chosen by the other side and is not verified. Only accept if you expect this.
        </span>
      </div>
      <p className="mt-2 text-footnote text-muted">
        {s.branding.approve_mode === "click" ? "Accept lets them in right away." : "Accept lets them in right away, no password needed. Typing your one-time password lets them in too."} Below: what they may do.
      </p>
      {/* At least three switches stay in view before Accept, however many banners sit above. */}
      <div className="min-h-[7.75rem] flex-1 overflow-y-auto">
        <Perms c={c} s={s} onError={onError} />
      </div>
      <div className="mt-3 flex gap-2">
        <Button className="flex-1" onClick={() => api.close(c.id)}>
          Decline
        </Button>
        {/* No autoFocus: the window takes the focus when a request arrives, and a key typed at that moment must not accept. */}
        <Button variant="primary" className="flex-1" disabled={s.blocked} onClick={() =>
            api.authorize(c.id).then(
              () => onError(`Accepted: ${c.name || formatId(c.peer_id)} is now connected`),
              (e) => onError(String(e)),
            )
          }
        >
          Accept
        </Button>
      </div>
    </Card>
  );
}

function useElapsed(since: number) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 1000);
    return () => clearInterval(t);
  }, []);
  const sec = Math.floor((Date.now() - since) / 1000);
  return `${String(Math.floor(sec / 60)).padStart(2, "0")}:${String(sec % 60).padStart(2, "0")}`;
}

function Indicator({ on, icon, label, tone = "accent" }: { on: boolean; icon?: ReactNode; label: string; tone?: "accent" | "danger" }) {
  if (!on) return null;
  return (
    <span className="inline-flex items-center gap-1.5 rounded-sm border border-hairline bg-parchment px-2 py-1 text-footnote">
      <span className={`inline-flex ${tone === "danger" ? "text-danger" : "text-accent"}`}>{icon ?? <span className="h-1.5 w-1.5 rounded-full bg-current" />}</span>
      {label}
    </span>
  );
}

function InSession({ c, files, s, since, rec, onError, msgs, onSend, stacked }: { c: Client; files: Client[]; s: State; since: number; rec: boolean; onError: (m: string) => void; msgs: Msg[]; onSend: (t: string) => void; stacked: boolean }) {
  const elapsed = useElapsed(since);
  const [view, setView] = useState<"perm" | "chat">("perm");
  const [seen, setSeen] = useState(0);
  const incoming = msgs.filter((m) => !m.me).length;
  // Each message from the technician opens the chat (the backend restores the window).
  useEffect(() => {
    if (incoming && chat) setView("chat");
  }, [incoming]);
  useEffect(() => {
    if (view === "chat") setSeen(msgs.length);
  }, [view, msgs.length]);
  const unread = msgs.slice(seen).filter((m) => !m.me).length;
  const chat = s.branding.chat;
  return (
    <Card className={`mt-2 flex flex-col !p-5 ${stacked ? "shrink-0" : "min-h-0 flex-1"}`}>
      <div className="flex items-center gap-3">
        <Avatar c={c} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold" title={c.name || undefined}>
            {c.name || formatId(c.peer_id)}
          </div>
          <div className="text-footnote text-muted">
            <span data-testid="qs-session-id" className="font-mono">ID {formatId(c.peer_id)}</span> · connected · {elapsed}
          </div>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        <Indicator on={!!c.keyboard && !c.is_file_transfer} icon={<Keyboard size={12} />} label="Control" />
        <Indicator on={files.length > 0 || c.is_file_transfer} icon={<FolderSync size={12} />} label="Transferring files" />
        {/* Set once the user allowed it; the technician may record from then on. */}
        <Indicator on={rec} label="Recording allowed" tone="danger" />
      </div>
      {chat && <div className="mt-3 flex gap-1 rounded-full bg-parchment p-1">
        {(["perm", "chat"] as const).map((v) => (
          <button
            key={v}
            data-testid={`qs-tab-${v}`}
            aria-pressed={view === v}
            onClick={() => setView(v)}
            className={`inline-flex h-8 flex-1 items-center justify-center gap-1.5 rounded-full text-caption ${view === v ? "bg-accent font-semibold text-on-accent" : "text-muted hover:text-ink"}`}
          >
            {v === "perm" ? "Permissions" : "Chat"}
            {v === "chat" && unread > 0 && (
              <span data-testid="qs-chat-unread" aria-label={`${unread} unread`} className="grid h-4 min-w-4 place-items-center rounded-full bg-danger px-1 text-footnote font-semibold leading-none text-white dark:text-black">
                {unread}
              </span>
            )}
          </button>
        ))}
      </div>}
      {view === "perm" || !chat ? (
        <div className={stacked ? "" : "min-h-0 flex-1 overflow-y-auto"}>
          <Perms c={c} s={s} onError={onError} also={files} />
        </div>
      ) : (
        <div className={`flex flex-col ${stacked ? "h-64" : "min-h-0 flex-1"}`}>
          <Chat msgs={msgs} onSend={onSend} />
        </div>
      )}
      <Button variant="disconnect" className="mt-3 w-full" onClick={() => [c, ...files].forEach((x) => api.close(x.id))}>
        Disconnect
      </Button>
    </Card>
  );
}

function Chat({ msgs, onSend }: { msgs: Msg[]; onSend: (t: string) => void }) {
  const [v, setV] = useState("");
  const end = useRef<HTMLDivElement>(null);
  // Braces: scrollIntoView may return a Promise, which React would call as cleanup.
  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [msgs.length]);
  return (
    <div className="mt-2 flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto py-1">
        {msgs.length === 0 && <div className="pt-6 text-center text-caption text-muted">Messages from your technician appear here.</div>}
        {msgs.map((m, i) => (
          <div key={i} className={`flex ${m.me ? "justify-end" : ""}`}>
            <div data-testid="qs-chat-msg" data-me={m.me} className={`max-w-[80%] select-text whitespace-pre-wrap break-words rounded-2xl px-3 py-1.5 text-caption ${m.me ? "bg-accent text-on-accent" : "bg-parchment"}`}>
              {m.text}
            </div>
          </div>
        ))}
        <div ref={end} />
      </div>
      <form
        className="mt-2 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (v.trim()) onSend(v.trim());
          setV("");
        }}
      >
        <input
          data-testid="qs-chat-input"
          aria-label="Message to your technician"
          value={v}
          onChange={(e) => setV(e.target.value)}
          placeholder="Reply"
          maxLength={4096}
          className="h-9 min-w-0 flex-1 rounded-xl border border-hairline bg-parchment px-3.5 text-caption focus-visible:border-focus focus-visible:outline-offset-0"
        />
        <button aria-label="Send" className="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-accent text-on-accent">
          <Send size={15} />
        </button>
      </form>
    </div>
  );
}

/** A Mac that has not allowed screen recording sends the technician no picture, silently. */
function MacBanner({ s, onError }: { s: State; onError: (m: string) => void }) {
  const m = s.mac;
  if (!m || (m.screen && m.accessibility)) return null;
  const fix = (w: "screen" | "accessibility") => api.macFix(w).catch((e) => onError(String(e)));
  return (
    <div role="alert" data-testid="qs-mac-perms" className="mb-2 rounded-xl border border-hairline bg-parchment px-3 py-2.5 text-caption">
      <div className="flex items-center gap-2 font-semibold">
        <Dot tone="warn" />
        {m.screen ? "Your technician cannot control this Mac yet" : "Your technician cannot see this screen yet"}
      </div>
      <div className="mt-0.5 text-footnote text-muted">
        Click Fix, switch TetherQS on in System Settings, then choose Quit &amp; Reopen. A switch that is already on can belong to an older version; Fix clears it.
      </div>
      <div className="mt-1.5 flex gap-3">
        {!m.screen && (
          <button className="text-caption text-accent" onClick={() => fix("screen")}>
            Fix Screen Recording
          </button>
        )}
        {!m.accessibility && (
          <button className="text-caption text-accent" onClick={() => fix("accessibility")}>
            Fix Accessibility
          </button>
        )}
      </div>
    </div>
  );
}

function UpdateBanner({ s, onError }: { s: State; onError: (m: string) => void }) {
  const u = s.update;
  const [now, setNow] = useState(Date.now() / 1000);
  useEffect(() => {
    if (!u.forced_at) return;
    const t = setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => clearInterval(t);
  }, [u.forced_at]);
  if (!s.blocked && !u.available) return null;
  const countdown = u.forced_at ? Math.max(0, Math.round(u.forced_at - now)) : null;
  return (
    <div className="mb-2 flex items-center gap-3 rounded-xl border border-hairline bg-parchment px-3 py-2.5">
      <Dot tone={s.blocked ? "warn" : "ok"} />
      <div className="min-w-0 flex-1 text-caption">
        {s.blocked ? "An update is required before your technician can connect." : `Version ${u.available} is available.`}
        {countdown !== null && <span className="text-muted"> Restarting in {countdown} s.</span>}
        {u.waiting_idle && countdown === null && <span className="text-muted"> Will update after this session.</span>}
        {u.error && <div className="text-footnote text-danger">{u.error}</div>}
      </div>
      {u.available && (
        <button className="shrink-0 text-caption text-accent disabled:opacity-40" disabled={u.installing} onClick={() => api.installUpdate().catch((e) => onError(String(e)))}>
          {u.installing ? (u.progress?.[1] ? `Updating… ${Math.floor((100 * u.progress[0]) / u.progress[1])} %` : "Updating…") : "Update"}
        </button>
      )}
    </div>
  );
}

function Footer({ s, onError }: { s: State; onError: (m: string) => void }) {
  const pill =
    s.clients.some((c) => c.authorized) ? (["ok", "Session active"] as const) : s.status === 1 ? (["ok", "Ready"] as const) : s.status === 0 ? (["warn", "Connecting"] as const) : (["danger", "Offline"] as const);
  return (
    <div className="mt-auto flex items-center gap-3 pt-4">
      <div data-testid="qs-status" className="inline-flex shrink-0 items-center gap-2 whitespace-nowrap rounded-full bg-parchment px-3 py-1.5 text-footnote">
        <Dot tone={pill[0]} />
        {pill[1]}
      </div>
      <div className="flex-1" />
      {s.branding.elevate &&
        (s.elevated ? (
          <span className="inline-flex items-center gap-1.5 text-footnote text-muted">
            <ShieldCheck size={14} className="text-ok" /> Admin access on
          </span>
        ) : (
          <div className="flex min-w-0 flex-col items-end gap-0.5 text-right">
            <button className="inline-flex items-center gap-1.5 text-caption text-accent" onClick={() => api.elevate().catch((e) => onError(String(e)))}>
              <ShieldCheck size={14} /> Allow admin access
            </button>
            <span className="text-footnote text-muted">Lets your technician handle admin prompts (UAC). Windows asks you once.</span>
          </div>
        ))}
    </div>
  );
}

function Help({ customApp, upload, status, lastCheck, stale, onBack }: { customApp: string | null; upload: boolean; status: number; lastCheck: string | null; stale?: boolean; onBack: () => void }) {
  const [d, setD] = useState<Diagnostics | null>(null);
  const [err, setErr] = useState("");
  // Per server; missing = checking. The local rows never wait for it.
  const [p, setP] = useState<Record<string, Probe>>({});
  const probe = (server: string) => {
    setP(({ [server]: _, ...rest }) => rest);
    api.diagnosticsProbe(server).then(
      (r) => setP((x) => ({ ...x, [server]: r })),
      (e) => setP((x) => ({ ...x, [server]: { reachable: false, ms: null, error: String(e) } })),
    );
  };
  useEffect(() => {
    api.diagnostics().then((d) => {
      setD(d);
      d.servers.forEach(probe);
    }, (e) => setErr(String(e)));
  }, []);
  const host = (u: string) => u.replace(/^https?:\/\//, "");
  const rows: [string, ReactNode][] = d
    ? [
        ...(customApp ? ([["Custom app", customApp]] as [string, ReactNode][]) : []),
        ["Version", d.version],
        ["Updates", <UpdateCheck last={lastCheck} stale={stale} />],
        ["ID", formatId(d.id)],
        ["Connection", status === 1 ? "Ready" : status === 0 ? "Connecting to the server…" : "Offline"],
        ...(d.servers.length ? d.servers : [""]).map((sv, i): [string, ReactNode] => [
          d.servers.length > 1 ? `Server ${i + 1}` : "Server",
          !sv ? (
            "—"
          ) : (
            <span className="flex flex-col gap-0.5">
              <span>{host(sv)}</span>
              {p[sv] ? (
                <span className="inline-flex flex-wrap items-center gap-1.5 text-muted">
                  <Dot tone={p[sv].reachable ? "ok" : "danger"} />
                  {p[sv].reachable ? `Reachable (${p[sv].ms} ms)` : p[sv].error ?? "Not reachable"}
                  <button className="text-accent" onClick={() => probe(sv)}>
                    Retry
                  </button>
                </span>
              ) : (
                <span className="text-muted">Checking…</span>
              )}
            </span>
          ),
        ]),
        ["Proxy", d.proxy],
        ["Admin access", d.elevated ? "On" : "Off"],
        ["Policy", d.policy_serial ? `#${d.policy_serial}${d.policy_group ? ` (${d.policy_group})` : ""}` : "Defaults"],
      ]
    : [];
  return (
    <div className="-mx-2 flex min-h-0 flex-1 flex-col overflow-y-auto px-2">
      <button className="mb-3 inline-flex items-center gap-1.5 self-start text-caption text-accent" onClick={onBack}>
        <ArrowLeft size={14} /> Back
      </button>
      <Card className="!p-5">
        <div className="mb-2 text-callout font-semibold">Diagnostics</div>
        {!d && <div className="text-caption text-muted">{err || "Loading…"}</div>}
        {rows.map(([k, v]) => (
          <div key={k} className="flex gap-3 border-t border-hairline py-2 text-caption first-of-type:border-t-0">
            <span className="w-32 shrink-0 text-muted">{k}</span>
            <span className="min-w-0 flex-1 select-text break-words">{v}</span>
          </div>
        ))}
      </Card>
      {upload && <SendDiagnostics />}
      <p className="mt-4 text-footnote text-muted">Admin prompts (UAC) are only visible to your technician after you allow admin access.</p>
    </div>
  );
}

/** Help › Send to support: the ticket code from support, then the diagnostics bundle (logs, settings without secrets). */
function SendDiagnostics() {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  return (
    <Card className="mt-3 !p-5">
      <div className="mb-1 text-callout font-semibold">Send to support</div>
      <p className="mb-2 text-caption text-muted">Enter the code your support contact gave you. Sends logs and settings, never passwords.</p>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          setBusy(true);
          setMsg(null);
          api.diagUpload(code).then(
            () => {
              setMsg({ ok: true, text: "Sent. Tell your support contact." });
              setCode("");
              setBusy(false);
            },
            (err) => {
              setMsg({ ok: false, text: String(err) });
              setBusy(false);
            },
          );
        }}
      >
        <input
          data-testid="qs-diag-code"
          aria-label="Support ticket code"
          value={code}
          onChange={(e) => setCode(e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 8))}
          placeholder="Ticket code"
          className="h-9 min-w-0 flex-1 rounded-xl border border-hairline bg-parchment px-3.5 font-mono text-caption tracking-wider focus-visible:border-focus focus-visible:outline-offset-0"
        />
        <Button variant="primary" className="!h-9" type="submit" disabled={busy || code.length !== 8}>
          {busy ? "Sending…" : "Send"}
        </Button>
      </form>
      {msg && (
        <p data-testid="qs-diag-result" role="status" className={`mt-2 text-caption ${msg.ok ? "text-ink-80" : "text-danger"}`}>
          {msg.text}
        </p>
      )}
    </Card>
  );
}

/** "Check for updates": the last result (also of the automatic check) and a button to check now. */
function UpdateCheck({ last, stale }: { last: string | null; stale?: boolean }) {
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const check = () => {
    setBusy(true);
    setErr(null);
    api.checkUpdate().then(
      () => setBusy(false),
      (e) => {
        setErr(`Check failed: ${e}`);
        setBusy(false);
      },
    );
  };
  const text = busy ? "Checking…" : (err ?? last ?? "Not checked yet");
  return (
    <span className="flex flex-col gap-0.5">
      <span data-testid="qs-update-check" className={text.startsWith("Check failed") ? "text-danger" : "text-muted"}>
        {text}
      </span>
      {stale && <span className="text-warn">Update information is out of date.</span>}
      <button className="self-start text-accent disabled:opacity-40" disabled={busy} onClick={check}>
        Check for updates
      </button>
    </span>
  );
}

/** Control, bidi, line-separator and zero-width characters out (the backend's `invisible` set in
 *  agent_elevated.rs, as in the desktop's AgentHere.tsx): they could reorder or hide what is read. */
function plainText(s: string): string {
  return s.replace(/[\u0000-\u001f\u007f-\u009f\u061c\u200b-\u200f\u2028-\u202e\u2060-\u2064\u2066-\u2069\ufeff]/g, "");
}

/** The health agent's pre-dialog (spec 11.2): only the person here answers it (remote input never
 *  reaches this window). Like the recording consent: Escape and the first, focused button decline. */
function AgentConsentDialog({ d, onAnswer }: { d: AgentConsent; onAnswer: (yes: boolean) => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (!ref.current?.open) ref.current?.showModal();
  }, []);
  return (
    <dialog
      ref={ref}
      aria-labelledby="qs-agent-title"
      aria-describedby="qs-agent-text"
      className="animate-rise m-auto max-h-[calc(100%-2rem)] w-[calc(100%-3rem)] max-w-none overflow-y-auto rounded-2xl border border-hairline bg-tile p-8 text-ink shadow-float backdrop:bg-black/40"
      onCancel={(e) => {
        e.preventDefault();
        onAnswer(false);
      }}
    >
      <h2 id="qs-agent-title" className="text-headline font-semibold">
        {plainText(d.title)}
      </h2>
      <div id="qs-agent-text">
        {d.paragraphs.map((p, i) => (
          <p key={i} className="mt-2 text-callout text-ink-80">
            {plainText(p)}
          </p>
        ))}
      </div>
      <div className="mt-6 flex gap-2">
        {/* First, with the focus: a key typed as the question appears must not install. */}
        <Button className="flex-1" autoFocus onClick={() => onAnswer(false)}>
          {plainText(d.no)}
        </Button>
        <Button variant="primary" className="flex-1" onClick={() => onAnswer(true)}>
          {plainText(d.yes)}
        </Button>
      </div>
    </dialog>
  );
}

function ConsentDialog({ c, fallback, onAnswer }: { c: Consent; fallback: string; onAnswer: (allow: boolean) => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  // Native modal: focus moves in and stays in, the page behind is inert.
  useEffect(() => {
    if (!ref.current?.open) ref.current?.showModal();
  }, []);
  return (
    <dialog
      ref={ref}
      aria-labelledby="qs-consent-title"
      className="animate-rise m-auto w-[calc(100%-3rem)] max-w-none rounded-2xl border border-hairline bg-tile p-8 text-ink shadow-float backdrop:bg-black/40"
      // Escape answers "Don't allow"; the dialog never closes without an answer.
      onCancel={(e) => {
        e.preventDefault();
        onAnswer(false);
      }}
    >
      <div className="mb-1 inline-flex items-center gap-2 text-footnote text-danger">
        <span className="h-1.5 w-1.5 rounded-full bg-danger" /> Recording
      </div>
      <div id="qs-consent-title" className="text-headline font-semibold">
        {c.peer_name || "Your technician"}
      </div>
      <p className="mt-2 text-callout text-ink-80">{c.text || fallback}</p>
      <div className="mt-6 flex gap-2">
        {/* First in the dialog, so it gets the focus: a key typed as the question appears must not allow. */}
        <Button className="flex-1" autoFocus onClick={() => onAnswer(false)}>
          Don't allow
        </Button>
        <Button variant="primary" className="flex-1" onClick={() => onAnswer(true)}>
          Allow
        </Button>
      </div>
    </dialog>
  );
}
