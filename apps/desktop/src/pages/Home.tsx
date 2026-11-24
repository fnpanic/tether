import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowRight, Copy, Eye, EyeOff, FolderSync, LayoutList, Lock, MoreHorizontal, RefreshCw, Search, Settings as SettingsIcon, Trash2, Video } from "lucide-react";
import { api, formatId, invoke, type Peer, type Status } from "../lib/ipc";
import { Button, IconButton, Logo, PlatformIcon, StatusDot, TitleBar, useToast } from "../components/ui";
import { Settings } from "./Settings";
import { UpdateCard } from "../components/UpdateCard";
import { AgentCard } from "../components/AgentHere";
import { errorText, usePolicy } from "../lib/policy";
import { useMode } from "../lib/mode";
import { Recordings, recordingsHidden, useRecordings } from "./Recordings";
import { RecordingSaved } from "../components/RecordingSaved";

function useStatus() {
  const [s, setS] = useState<Status | null>(null);
  const refresh = useCallback(() => api.status().then(setS).catch(() => {}), []);
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 1500);
    return () => clearInterval(t);
  }, [refresh]);
  return { s, refresh };
}

function StatusPill({ s }: { s: Status | null }) {
  const ready = s && s.status === 1 && s.key_confirmed;
  const connecting = s && s.status === 0;
  const [tone, text] = ready ? (["ok", "Ready"] as const) : connecting ? (["warn", "Connecting"] as const) : (["danger", "Offline"] as const);
  return (
    <div className="no-drag flex items-center gap-2 rounded-full bg-parchment px-3 py-1 text-footnote text-muted" title={ready ? "Connected to your server. Others can reach this device." : undefined}>
      <StatusDot tone={tone} />
      {text}
    </div>
  );
}

function ThisDevice({ s, onToast, onRefresh }: { s: Status | null; onToast: (m: string) => void; onRefresh: () => void }) {
  const [reveal, setReveal] = useState(false);
  const copy = (v: string, what: string) => navigator.clipboard.writeText(v).then(() => onToast(`${what} copied`));
  return (
    <section className="flex flex-col rounded-2xl border border-hairline bg-tile p-8 short:p-6">
      <div className="text-caption text-muted">This device</div>
      <div className="mt-2 flex items-center gap-2">
        <div className="font-mono text-display font-semibold tabular-nums" aria-label="Your ID">
          {s?.id ? formatId(s.id) : <span className="text-faint">··· ··· ···</span>}
        </div>
        <IconButton label="Copy ID" onClick={() => s && copy(s.id, "ID")}>
          <Copy size={17} />
        </IconButton>
      </div>

      <div className="mt-8 text-caption text-muted short:mt-4">One-time password</div>
      <div className="mt-1 flex items-center gap-1">
        <div className="mr-2 min-w-[7.5rem] font-mono text-headline tabular-nums tracking-[0.12em]" aria-label="One-time password">
          {s?.password ? (reveal ? s.password : "•".repeat(s.password.length)) : "—"}
        </div>
        <IconButton label={reveal ? "Hide password" : "Show password"} onClick={() => setReveal(!reveal)}>
          {reveal ? <EyeOff size={17} /> : <Eye size={17} />}
        </IconButton>
        <IconButton label="Copy password" onClick={() => s && copy(s.password, "Password")}>
          <Copy size={16} />
        </IconButton>
        <IconButton
          label="New password"
          onClick={async () => {
            await api.refreshPassword();
            onRefresh();
            onToast("New password generated");
          }}
        >
          <RefreshCw size={16} />
        </IconButton>
      </div>

      <div className="mt-auto flex items-center gap-2 pt-8 text-caption text-muted short:pt-4">
        <Lock size={14} />
        End-to-end encrypted. Nobody gets in without your approval.
      </div>
    </section>
  );
}

function ConnectBox({ peers, onConnect }: { peers: Peer[]; onConnect: (id: string, conn?: string) => void }) {
  const [v, setV] = useState("");
  const [focus, setFocus] = useState(false);
  const ref = useRef<HTMLInputElement>(null);
  const clean = v.replace(/\s+/g, "");
  const valid = /^\d{6,12}$/.test(clean) || /^[A-Za-z0-9_.@:-]{6,64}$/.test(clean);
  const suggestions = useMemo(
    () => (clean ? peers.filter((p) => p.id.startsWith(clean) || p.hostname.toLowerCase().includes(clean.toLowerCase())).slice(0, 5) : []),
    [clean, peers],
  );
  useEffect(() => {
    ref.current?.focus();
  }, []);
  const go = (id = clean, conn?: string) => id && onConnect(id, conn);

  return (
    <section className="flex flex-col rounded-2xl border border-hairline bg-tile p-8 short:p-6">
      <div className="text-caption text-muted">Remote device</div>
      <form
        className="relative mt-3"
        onSubmit={(e) => {
          e.preventDefault();
          valid && go();
        }}
      >
        <input
          ref={ref}
          value={/^\d[\d\s]*$/.test(v) ? formatId(v) : v}
          onChange={(e) => setV(e.target.value.slice(0, 64))}
          onFocus={() => setFocus(true)}
          onBlur={() => setTimeout(() => setFocus(false), 120)}
          placeholder="Enter ID"
          aria-label="Remote device ID"
          spellCheck={false}
          autoComplete="off"
          className="h-14 w-full rounded-full border border-hairline bg-parchment px-6 font-mono text-title tabular-nums text-ink outline-none placeholder:font-sans placeholder:text-body placeholder:text-faint focus-visible:border-focus"
        />
        {focus && suggestions.length > 0 && (
          <div className="animate-fade absolute inset-x-0 top-[62px] z-10 overflow-hidden rounded-2xl border border-hairline bg-tile p-1 shadow-float">
            {suggestions.map((p) => (
              <button type="button" key={p.id} onMouseDown={() => go(p.id)} className="flex h-10 w-full items-center gap-3 rounded-md px-3 text-left hover:bg-tile-2">
                <span className="text-muted">
                  <PlatformIcon platform={p.platform} size={16} />
                </span>
                <span className="font-mono text-caption">{formatId(p.id)}</span>
                <span className="truncate text-caption text-muted" title={p.hostname}>
                  {p.hostname}
                </span>
              </button>
            ))}
          </div>
        )}
        <div className="mt-4 flex gap-2">
          <Button type="submit" variant="primary" size="lg" className="flex-1" disabled={!valid}>
            Connect <ArrowRight size={17} />
          </Button>
          <Button type="button" variant="secondary" size="lg" disabled={!valid} onClick={() => go(clean, "file-transfer")} title="Transfer files" aria-label="Transfer files">
            <FolderSync size={17} />
          </Button>
        </div>
      </form>
      <p className="mt-auto pt-8 text-caption text-muted short:pt-4">Ask for their ID. They confirm the request on their screen, or give you their password.</p>
    </section>
  );
}

function PeerCard({ p, onConnect, onRemove }: { p: Peer; onConnect: () => void; onRemove: () => void }) {
  const [menu, setMenu] = useState(false);
  return (
    <div className="group animate-rise relative">
      <button
        onClick={onConnect}
        className="flex w-full flex-col rounded-2xl border border-hairline bg-tile p-3 text-left hover:bg-tile-2"
        aria-label={`Connect to ${p.hostname || p.id}`}
      >
        <div className="grid aspect-[16/10] w-full place-items-center rounded-md bg-parchment text-muted group-hover:text-accent">
          <PlatformIcon platform={p.platform} size={28} />
        </div>
        <div className="mt-3 truncate px-1 text-caption font-semibold" title={p.hostname || p.username || undefined}>
          {p.hostname || p.username || "Unknown device"}
        </div>
        <div className="px-1 pb-1 font-mono text-footnote text-muted">{formatId(p.id)}</div>
      </button>
      <button
        aria-label="More"
        onClick={() => setMenu(!menu)}
        className="absolute right-5 top-5 grid h-8 w-8 place-items-center rounded-full bg-tile/80 text-muted opacity-0 backdrop-blur hover:text-ink group-focus-within:opacity-100 group-hover:opacity-100"
      >
        <MoreHorizontal size={16} />
      </button>
      {menu && (
        <div className="animate-fade absolute right-5 top-14 z-10 w-max min-w-48 overflow-hidden rounded-xl border border-hairline bg-tile p-1 shadow-float" onMouseLeave={() => setMenu(false)}>
          <button className="flex h-9 w-full items-center gap-2 whitespace-nowrap rounded-md px-3 text-caption text-danger hover:bg-tile-2" onClick={onRemove}>
            <Trash2 size={15} /> Remove from recents
          </button>
        </div>
      )}
    </div>
  );
}

/** A Mac that has not allowed Tether these sends a technician no picture / ignores input, silently. */
function MacPermissions() {
  const [m, setM] = useState<{ screen: boolean; accessibility: boolean } | null>(null);
  useEffect(() => {
    const load = () => invoke<{ screen: boolean; accessibility: boolean } | null>("mac_permissions").then(setM).catch(() => {});
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);
  if (!m || (m.screen && m.accessibility)) return null;
  const fix = (which: string) => invoke("mac_fix", { which }).catch(() => {});
  return (
    <div role="alert" className="mb-6 rounded-2xl border border-warn/40 bg-warn/10 px-5 py-4">
      <div className="text-callout font-semibold">{m.screen ? "A technician cannot control this Mac yet" : "A technician cannot see this Mac's screen yet"}</div>
      <div className="mt-1 text-caption text-muted">
        Click Fix, switch Tether on in System Settings, then choose Quit &amp; Reopen. A switch that is already on can belong to an older version; Fix clears it.
      </div>
      <div className="mt-3 flex gap-2">
        {!m.screen && (
          <Button size="sm" variant="secondary" onClick={() => fix("screen")}>
            Fix Screen Recording
          </Button>
        )}
        {!m.accessibility && (
          <Button size="sm" variant="secondary" onClick={() => fix("accessibility")}>
            Fix Accessibility
          </Button>
        )}
      </div>
    </div>
  );
}

export function Home() {
  const { s, refresh } = useStatus();
  const [peers, setPeers] = useState<Peer[]>([]);
  const [q, setQ] = useState("");
  const [settings, setSettings] = useState(false);
  const [recordings, setRecordings] = useState(false);
  const rec = useRecordings();
  const eff = usePolicy();
  const toast = useToast();
  const mode = useMode();

  const loadPeers = useCallback(() => api.recentPeers().then(setPeers).catch(() => {}), []);
  useEffect(() => {
    loadPeers();
    const f = () => loadPeers();
    window.addEventListener("focus", f);
    return () => window.removeEventListener("focus", f);
  }, [loadPeers]);

  const connect = (id: string, conn?: string) => {
    api.openSession(id, conn).catch((e) => toast.show(errorText(e)));
  };
  const shown = peers.filter((p) => !q || p.id.includes(q.replace(/\s/g, "")) || `${p.hostname} ${p.username}`.toLowerCase().includes(q.toLowerCase()));

  return (
    <div className="flex h-full flex-col overflow-hidden bg-canvas">
      <TitleBar>
        <Logo size={20} />
        <span className="text-caption font-semibold">Tether</span>
        <div className="flex-1" data-tauri-drag-region />
        <StatusPill s={s} />
        {mode.policy !== "off" && (
          <Button size="sm" variant="ghost" className="no-drag" onClick={() => mode.set("advanced")} title="Advanced mode: all hosts with search, tags and sessions">
            <LayoutList size={16} /> Hosts
          </Button>
        )}
        {!recordingsHidden(eff, rec.st) && (
          <IconButton label="Recordings" className="no-drag" onClick={() => setRecordings(true)}>
            <Video size={17} />
          </IconButton>
        )}
        <IconButton label="Settings" className="no-drag" onClick={() => setSettings(true)}>
          <SettingsIcon size={17} />
        </IconButton>
      </TitleBar>

      <main className="flex min-h-0 flex-1 flex-col overflow-y-auto px-8 pb-6 pt-4">
        <div className="mx-auto flex min-h-0 w-full max-w-[980px] flex-1 flex-col short:min-h-fit">
          <MacPermissions />
          <AgentCard />
          <div className="grid shrink-0 grid-cols-[1.1fr_1fr] gap-6">
            <ThisDevice s={s} onToast={toast.show} onRefresh={refresh} />
            <ConnectBox peers={peers} onConnect={connect} />
          </div>

          <div className="mt-8 flex shrink-0 items-end gap-3 short:mt-6">
            <h2 className="text-headline font-semibold">Recent</h2>
            <div className="flex-1" />
            {peers.length > 4 && (
              <label className="flex h-9 items-center gap-2 rounded-full border border-hairline bg-parchment px-3.5 text-muted focus-within:border-focus">
                <Search size={14} />
                <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" aria-label="Search recent devices" className="w-40 bg-transparent text-caption text-ink outline-none placeholder:text-faint" />
              </label>
            )}
          </div>

          {peers.length === 0 ? (
            <div className="mt-4 rounded-2xl bg-parchment px-6 py-12 text-center short:py-8">
              <div className="text-callout font-semibold">No sessions yet</div>
              <div className="mt-1 text-caption text-muted">Devices you connect to appear here.</div>
            </div>
          ) : (
            <div className="-mx-2 mt-2 grid min-h-0 flex-1 grid-cols-[repeat(auto-fill,minmax(200px,1fr))] content-start gap-6 overflow-y-auto p-2 short:flex-none short:overflow-visible">
              {shown.map((p) => (
                <PeerCard
                  key={p.id}
                  p={p}
                  onConnect={() => connect(p.id)}
                  onRemove={async () => {
                    await api.removePeer(p.id);
                    loadPeers();
                  }}
                />
              ))}
            </div>
          )}
        </div>
      </main>
      {recordings && <Recordings onClose={() => setRecordings(false)} onToast={toast.show} />}
      {settings && <Settings onClose={() => setSettings(false)} status={s} />}
      <RecordingSaved />
      <UpdateCard />
      {toast.node}
    </div>
  );
}
