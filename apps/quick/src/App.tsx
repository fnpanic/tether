// TetherQS: one 420×560 window. Visual rules: /DESIGN.md.
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useRef, useState } from "react";
import { ArrowLeft, Check, Copy, Eye, EyeOff, FolderSync, HelpCircle, Keyboard, Minus, RefreshCw, ShieldAlert, ShieldCheck, X } from "lucide-react";
import { api, type Client, type Diagnostics, formatId, type Probe, inTauri, onCm, type State } from "./ipc";

type Consent = { conn_id: number; peer_name: string; text: string };

async function win() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export function App() {
  const [s, setS] = useState<State | null>(null);
  const [help, setHelp] = useState(false);
  const [consent, setConsent] = useState<Consent | null>(null);
  const [rec, setRec] = useState<Set<number>>(new Set());
  const [toast, setToast] = useState<string | null>(null);
  const since = useRef(new Map<number, number>());

  const refresh = () => api.state().then(setS).catch(() => {});
  const say = (m: string) => {
    setToast(m);
    setTimeout(() => setToast(null), 2500);
  };

  useEffect(() => {
    if (!inTauri) return;
    refresh();
    const t = setInterval(refresh, 1000);
    const un = onCm((name, p) => {
      if (name === "record_consent_request") setConsent(p);
      else if (name === "refused") say(p.reason === "update-required" ? "A connection was refused: update TetherQS first." : "Incoming sessions are turned off by your administrator.");
      refresh();
    });
    return () => {
      clearInterval(t);
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!s) return;
    const r = document.documentElement.style;
    r.setProperty("--accent", s.branding.accent);
    r.setProperty("--focus", s.branding.accent);
    document.title = s.branding.title;
  }, [s?.branding.accent, s?.branding.title]);

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
      <div className="flex min-h-0 flex-1 flex-col px-6 pb-5">
        {help ? (
          <Help customApp={s.branding.custom_app} status={s.status} onBack={() => setHelp(false)} />
        ) : (
          <>
            <UpdateBanner s={s} />
            {pending ? (
              <Request c={pending} s={s} onError={say} />
            ) : active.length ? (
              active.map((c) => <InSession key={c.id} c={c} files={companions(c)} s={s} since={since.current.get(c.id)!} rec={rec.has(c.id)} onError={say} />)
            ) : (
              <Idle s={s} onCopied={say} />
            )}
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
      {toast && (
        <div role="status" className="pointer-events-none fixed inset-x-0 bottom-6 z-50 flex justify-center px-6">
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
      <span data-tauri-drag-region data-testid="qs-title" className="min-w-0 flex-1 truncate text-caption font-semibold">
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
      <button aria-label="Close" className={`${btn} hover:!bg-danger hover:!text-white`} onClick={async () => (await win()).close()}>
        <X size={16} />
      </button>
    </div>
  );
}

function Button({ variant = "pearl", className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "pearl" | "secondary" | "disconnect" }) {
  const v = {
    primary: "rounded-full bg-accent text-white hover:brightness-110",
    secondary: "rounded-full border border-accent text-accent hover:bg-accent/10",
    pearl: "rounded-xl border border-hairline bg-tile-2 text-ink-80 hover:bg-parchment",
    // DESIGN.md: the in-session disconnect is the one red control.
    disconnect: "rounded-full bg-danger text-white hover:brightness-110",
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
      className={`relative h-6 w-10 shrink-0 rounded-full disabled:opacity-40 ${checked ? "bg-accent" : "bg-hairline"}`}
    >
      <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all duration-150 ${checked ? "left-[18px]" : "left-0.5"}`} style={{ boxShadow: "0 1px 3px rgba(0,0,0,.25)" }} />
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
          <span data-testid="qs-id" className="flex-1 select-text font-mono text-[34px] font-semibold leading-tight tracking-tight">
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
  return <div className="grid h-11 w-11 shrink-0 place-items-center rounded-full bg-accent text-callout font-semibold text-white">{initials}</div>;
}

function Perms({ c, s, onError, also = [] }: { c: Client; s: State; onError: (m: string) => void; also?: Client[] }) {
  return (
    <div className="mt-3 space-y-0.5">
      {s.branding.perms.map((p) => (
        <div key={p.name} className="flex h-9 items-center gap-3 rounded-md px-1">
          <span className={`flex-1 text-caption ${p.locked && !p.allowed ? "text-faint" : ""}`}>{p.label}</span>
          <Toggle
            label={p.label}
            checked={!!c[p.name] && p.allowed}
            disabled={p.locked && !p.allowed}
            onChange={(v) => {
              [c, ...also].forEach((x) => api.permission(x.id, p.name, v).catch((e) => onError(String(e))));
              // A running file transfer does not check the switch: Files off ends it.
              if (p.name === "file" && !v) also.forEach((x) => api.close(x.id));
            }}
          />
        </div>
      ))}
    </div>
  );
}

function Request({ c, s, onError }: { c: Client; s: State; onError: (m: string) => void }) {
  return (
    <Card className="mt-2 flex min-h-0 flex-1 flex-col !p-5">
      <div className="flex items-center gap-3">
        <Avatar c={c} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold">{c.name || "Unknown"}</div>
          <div className="font-mono text-footnote text-muted">ID {formatId(c.peer_id)}</div>
        </div>
      </div>
      <div className="mt-3 flex items-start gap-2 rounded-xl bg-warn/10 px-3 py-2.5 text-caption">
        <ShieldAlert size={16} className="mt-0.5 shrink-0 text-warn" />
        <span>
          <b className="font-semibold">{c.name || "Someone"}</b> {c.is_file_transfer ? "wants to transfer files with this computer." : "wants to see and control this screen."} Only accept if you know this person.
        </span>
      </div>
      <p className="mt-2 text-footnote text-muted">
        {s.branding.approve_mode === "click" ? "Accept lets them in right away." : "Accept lets them in right away, no password needed. Typing your one-time password lets them in too."} Below: what they may do.
      </p>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Perms c={c} s={s} onError={onError} />
      </div>
      <div className="mt-3 flex gap-2">
        <Button className="flex-1" onClick={() => api.close(c.id)}>
          Decline
        </Button>
        <Button variant="primary" className="flex-1" autoFocus disabled={s.blocked} onClick={() =>
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
    <span className={`inline-flex items-center gap-1.5 rounded-sm px-2 py-1 text-footnote ${tone === "danger" ? "bg-danger/10 text-danger" : "bg-accent/10 text-accent"}`}>
      {icon ?? <span className="h-1.5 w-1.5 rounded-full bg-danger" />}
      {label}
    </span>
  );
}

function InSession({ c, files, s, since, rec, onError }: { c: Client; files: Client[]; s: State; since: number; rec: boolean; onError: (m: string) => void }) {
  const elapsed = useElapsed(since);
  return (
    <Card className="mt-2 flex min-h-0 flex-1 flex-col !p-5">
      <div className="flex items-center gap-3">
        <Avatar c={c} />
        <div className="min-w-0 flex-1">
          <div className="truncate text-callout font-semibold">{c.name || formatId(c.peer_id)}</div>
          <div className="text-footnote text-muted">is connected · {elapsed}</div>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        <Indicator on={!!c.keyboard && !c.is_file_transfer} icon={<Keyboard size={12} />} label="Control" />
        <Indicator on={files.length > 0 || c.is_file_transfer} icon={<FolderSync size={12} />} label="Transferring files" />
        <Indicator on={rec} label="REC" tone="danger" />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Perms c={c} s={s} onError={onError} also={files} />
      </div>
      <Button variant="disconnect" className="mt-3 w-full" onClick={() => [c, ...files].forEach((x) => api.close(x.id))}>
        Disconnect
      </Button>
    </Card>
  );
}

function UpdateBanner({ s }: { s: State }) {
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
        {u.error && <div className="text-footnote text-danger">{u.error}</div>}
      </div>
      {u.available && (
        <button className="shrink-0 text-caption text-accent disabled:opacity-40" disabled={u.installing} onClick={() => api.installUpdate()}>
          {u.installing ? "Updating…" : "Update"}
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
      <div data-testid="qs-status" className="inline-flex items-center gap-2 rounded-full bg-parchment px-3 py-1.5 text-footnote">
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
          <button
            className="inline-flex items-center gap-1.5 text-caption text-accent"
            title="Lets your technician handle admin prompts (UAC). Windows asks you once."
            onClick={() => api.elevate().catch((e) => onError(String(e)))}
          >
            <ShieldCheck size={14} /> Allow admin access
          </button>
        ))}
    </div>
  );
}

function Help({ customApp, status, onBack }: { customApp: string | null; status: number; onBack: () => void }) {
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
    <div className="flex min-h-0 flex-1 flex-col">
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
      <p className="mt-4 text-footnote text-muted">Admin prompts (UAC) are only visible to your technician after you allow admin access.</p>
    </div>
  );
}

function ConsentDialog({ c, fallback, onAnswer }: { c: Consent; fallback: string; onAnswer: (allow: boolean) => void }) {
  return (
    <div className="animate-fade fixed inset-0 z-40 grid place-items-center bg-black/40 px-6">
      <div role="dialog" aria-modal className="animate-rise w-full rounded-2xl border border-hairline bg-tile p-8 shadow-float">
        <div className="mb-1 inline-flex items-center gap-2 text-footnote text-danger">
          <span className="h-1.5 w-1.5 rounded-full bg-danger" /> Recording
        </div>
        <div className="text-headline font-semibold">{c.peer_name || "Your technician"}</div>
        <p className="mt-2 text-callout text-ink-80">{c.text || fallback}</p>
        <div className="mt-6 flex gap-2">
          <Button className="flex-1" onClick={() => onAnswer(false)}>
            Don't allow
          </Button>
          <Button variant="primary" className="flex-1" onClick={() => onAnswer(true)}>
            Allow
          </Button>
        </div>
      </div>
    </div>
  );
}
