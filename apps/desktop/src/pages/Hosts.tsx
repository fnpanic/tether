// Advanced mode: the host inventory for technicians with hundreds of machines (docs/advanced.md).
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Circle,
  Clock,
  Download,
  Eye,
  FolderSync,
  HeartPulse,
  HelpCircle,
  LayoutList,
  Monitor,
  Palette as PaletteIcon,
  Pin,
  Plus,
  Search,
  Settings as SettingsIcon,
  Star,
  Tag,
  Trash2,
  Upload,
  Users,
  Video,
  Wand2,
  X,
} from "lucide-react";
import { Button, IconButton, Logo, Modal, SELECTED, TitleBar, useToast } from "../components/ui";
import { api, formatId, invoke, type Status } from "../lib/ipc";
import { useMode } from "../lib/mode";
import { Settings } from "./Settings";
import { Recordings, recordingsHidden, useRecordings } from "./Recordings";
import { RecordingSaved } from "../components/RecordingSaved";
import { UpdateCard } from "../components/UpdateCard";
import { errorText, isOn, usePolicy } from "../lib/policy";
import { buildHosts, label, type Host } from "../lib/hosts/model";
import { linkHolder } from "../lib/hosts/health";
import { healthAlerts, type Sent } from "../lib/hosts/notify";
import { applyRules } from "../lib/hosts/rules";
import { parse, search } from "../lib/hosts/query";
import { asOf, chipLines, faultsHint, lookupFor, NEGATED_NOTE, negatedSoftware, runnable, searcher, softwareTerms, termsKey, type SwState } from "../lib/hosts/software";
import { actions, flush, useHosts, useReadOnly } from "../lib/hosts/store";
import { HostTable, smartOrder, sortHosts, type Sort } from "../components/hosts/HostTable";
import { Detail } from "../components/hosts/Detail";
import { Palette, type Cmd } from "../components/hosts/Palette";
import { AddHost, BulkTags, Export, Help, Import, Rules, SaveSearch, Sessions, TagColors, Team } from "../components/hosts/Dialogs";
import { Kbd, TagDot } from "../components/hosts/bits";

type Dlg = null | "tags" | "rules" | "export" | "import" | "team" | "add" | "save" | "sessions" | "help" | "colors" | "remove";
type Kind = "default" | "file-transfer" | "view-only";

const perf = (globalThis as { __hostsPerf?: { last: number; samples: number[] } }).__hostsPerf ?? { last: 0, samples: [] };
(globalThis as { __hostsPerf?: typeof perf }).__hostsPerf = perf;

const isText = (t: EventTarget | null) => t instanceof HTMLElement && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable) && (t as HTMLInputElement).type !== "checkbox";

/** Tags in use with counts, grouped by namespace (`site:berlin` → site ▸ berlin). */
function tagTree(hosts: Host[]) {
  const count = new Map<string, number>();
  for (const h of hosts) {
    const seen = new Set<string>();
    for (const t of h.auto.length ? h.tags.concat(h.auto) : h.tags) {
      for (let i = t.indexOf(":"); ; i = t.indexOf(":", i + 1)) {
        const k = i < 0 ? t : t.slice(0, i);
        if (!seen.has(k)) {
          seen.add(k);
          count.set(k, (count.get(k) ?? 0) + 1);
        }
        if (i < 0) break;
      }
    }
  }
  const roots = [...count.keys()].filter((k) => !k.includes(":")).sort();
  const children = (p: string) => [...count.keys()].filter((k) => k.startsWith(p + ":") && !k.slice(p.length + 1).includes(":")).sort();
  return { count, roots, children };
}

export function Hosts() {
  const st = useHosts();
  const ro = useReadOnly();
  const eff = usePolicy();
  const toast = useToast();
  const canConnect = isOn(eff, "connect.outgoing");
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<Sort>({ key: "smart", desc: false });
  const [cursor, setCursor] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [dlg, setDlg] = useState<Dlg>(null);
  const [palette, setPalette] = useState(false);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const searchRef = useRef<HTMLInputElement>(null);
  const mode = useMode();
  const rec = useRecordings();
  const [settings, setSettings] = useState(false);
  const [recordings, setRecordings] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  useEffect(() => {
    const load = () => api.status().then(setStatus, () => {});
    load();
    const t = setInterval(load, 5000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    void actions.load().then(() => actions.refreshOnline());
    const online = setInterval(() => document.visibilityState === "visible" && useHosts.getState().onlineState !== "unsupported" && actions.refreshOnline(), 60_000);
    const team = setInterval(() => useHosts.getState().store.team?.enabled && actions.teamRefresh(), 5 * 60_000);
    const focus = () => {
      void actions.reloadPeers();
      if (useHosts.getState().onlineState !== "unsupported") void actions.refreshOnline();
    };
    const bye = () => void flush();
    window.addEventListener("focus", focus);
    window.addEventListener("beforeunload", bye);
    return () => {
      clearInterval(online);
      clearInterval(team);
      window.removeEventListener("focus", focus);
      window.removeEventListener("beforeunload", bye);
    };
  }, []);

  const hosts = useMemo(() => {
    const now = Date.now() / 1000;
    const team = st.store.team?.enabled;
    const list = buildHosts(st.store, st.peers, team ? st.org : [], st.online, now, team ? st.status : []);
    applyRules(list, st.store.rules, now);
    return list;
  }, [st.store, st.peers, st.org, st.online, st.status]);
  const byId = useMemo(() => new Map(hosts.map((h) => [h.id, h])), [hosts]);
  const hasHealth = useMemo(() => hosts.some((h) => h.status || h.conflict), [hosts]);

  // Health notifications (spec 10.5) after each status refresh; the first one only records states.
  // Personal switch (store.team.healthNotify, default on). Sent through the backend (Windows toasts).
  const sent = useRef<Sent | null>(null);
  const hostsRef = useRef(hosts);
  hostsRef.current = hosts;
  useEffect(() => {
    if (!st.status.length) return; // no data (yet, or a failed refresh): nothing changed state
    const r = healthAlerts(sent.current, hostsRef.current, Date.now() / 1000);
    sent.current = r.next;
    if (useHosts.getState().store.team?.healthNotify === false) return;
    for (const a of r.alerts) void invoke("health_notify", { title: a.title, body: a.body }).catch(() => {});
  }, [st.status]);
  const tree = useMemo(() => tagTree(hosts), [hosts]);
  const known = useMemo(() => [...tree.count.keys()].sort(), [tree]);
  const parsed = useMemo(() => parse(q), [q]);

  // software: terms ask the relays (spec 10.4): on Enter or after 800 ms without typing. Until the
  // answer for exactly these terms is in, a software term is unknown for every host: it matches
  // nothing, with or without `-` (an OR's other branch still can).
  const [sw, setSw] = useState<SwState>({ key: "", results: null });
  const finder = useMemo(() => searcher((c, a) => invoke(c, a), () => useHosts.getState().statusRelays, setSw), []);
  useEffect(() => () => finder.dispose(), [finder]);
  useEffect(() => finder.typed(q), [finder, q]);
  /** A query chosen, not typed (saved search, library, palette): searched at once. */
  const openQuery = useCallback((x: string) => (setQ(x), finder.enter(x)), [finder]);
  const swTerms = useMemo(() => softwareTerms(parsed.node), [parsed]);
  const swReady = runnable(swTerms);
  const swApplied = swReady.length > 0 && sw.key === termsKey(swReady) ? sw.results : null;
  const swRetry = !!swApplied && [...swApplied.values()].some((m) => [...m.values()].some((a) => !a.ok && ["busy", "limit", "unavailable", "overloaded", "offline", "error"].includes(a.why)));
  const swBusy = swReady.length > 0 && sw.key === termsKey(swReady) ? sw.busy : undefined;
  const swLines = useMemo(() => {
    if (!swTerms.length) return [];
    return [...chipLines(swTerms, swApplied, hosts, st.statusRelays, swBusy), ...(negatedSoftware(parsed.node) ? [NEGATED_NOTE] : [])];
  }, [swTerms, swApplied, swBusy, hosts, st.statusRelays, parsed]);
  const checkHint = useMemo(() => faultsHint(parsed.node, hosts), [parsed, hosts]);

  const rows = useMemo(() => {
    const t0 = performance.now();
    // A software term not answered yet (or not sendable) matches nothing, with or without `-`.
    const found = search(hosts, parsed, Date.now() / 1000, false, swApplied ? lookupFor(swApplied) : undefined);
    const r = sort.key === "smart" ? (parsed.words.length ? found : smartOrder(found)) : sortHosts(found, sort);
    perf.last = performance.now() - t0;
    perf.samples.push(perf.last);
    if (perf.samples.length > 500) perf.samples.shift();
    return r;
  }, [hosts, parsed, sort, swApplied]);

  const counts = useMemo(
    () => ({
      online: hosts.filter((h) => h.online === "on").length,
      pinned: hosts.filter((h) => h.pinned).length,
      recent: hosts.filter((h) => h.recent).length,
      org: hosts.filter((h) => h.org).length,
      untagged: hosts.filter((h) => !h.tags.length && !h.auto.length).length,
      attention: hosts.filter((h) => h.health === "crit" || h.health === "stale" || h.health === "conflict").length,
    }),
    [hosts],
  );
  // A saved search with a software: term has no count until it is opened (the relays answer it).
  const searchCounts = useMemo(() => new Map(st.store.searches.map((s) => [s.id, softwareTerms(parse(s.query).node).length ? undefined : search(hosts, s.query).length])), [hosts, st.store.searches]);

  // The cursor stays on a shown row.
  const cur = cursor && rows.some((h) => h.id === cursor) ? cursor : (rows[0]?.id ?? null);
  const curHost = cur ? byId.get(cur) : undefined;
  const nameOf = useCallback((id: string) => {
    const x = byId.get(id);
    return x && !x.agentOnly && label(x) !== id ? label(x) : undefined;
  }, [byId]);
  const holderOf = useCallback((id: string) => linkHolder(byId.get(id)), [byId]);
  const sel = useMemo(() => [...selected].filter((id) => byId.has(id)), [selected, byId]);
  // Tags, pin and Remove: agent-only rows (`m:<uuid>`) are not hosts of the list.
  const targets = (sel.length ? sel : cur ? [cur] : []).filter((id) => !id.startsWith("m:"));

  const connect = useCallback(
    (id: string, kind: Kind) => {
      if (id.startsWith("m:")) return toast.show("No remote access: Tether is not installed on this machine.", 3000);
      if (!canConnect) return toast.show("Your organisation does not allow connecting to other devices.", 3000);
      invoke<string>("open_session", { peer: id, conn: kind === "file-transfer" ? "file-transfer" : "default", viewOnly: kind === "view-only" }).then(
        () => actions.logSession(id, kind),
        (e) => toast.show(errorText(e), 3000),
      );
    },
    [canConnect, toast],
  );

  const move = (to: number, extend: boolean) => {
    if (!rows.length) return;
    const i = Math.max(0, Math.min(rows.length - 1, to));
    const id = rows[i].id;
    setCursor(id);
    if (extend) {
      const a = rows.findIndex((h) => h.id === (anchor ?? cur));
      const [x, y] = a < i ? [a, i] : [i, a];
      setSelected(new Set(rows.slice(Math.max(0, x), y + 1).map((h) => h.id)));
    } else setAnchor(id);
  };
  const curIdx = cur ? rows.findIndex((h) => h.id === cur) : -1;
  const page = Math.max(1, Math.floor((window.innerHeight - 200) / 36));

  const onRow = useCallback(
    (id: string, e: React.MouseEvent) => {
      if (e.button !== 0) return;
      setCursor(id);
      if (e.shiftKey) {
        const a = rows.findIndex((h) => h.id === (anchor ?? id));
        const b = rows.findIndex((h) => h.id === id);
        const [x, y] = a < b ? [a, b] : [b, a];
        setSelected(new Set(rows.slice(Math.max(0, x), y + 1).map((h) => h.id)));
      } else if (e.ctrlKey || e.metaKey) {
        setSelected((s) => {
          const n = new Set(s);
          if (n.has(id)) n.delete(id);
          else n.add(id);
          return n;
        });
        setAnchor(id);
      } else {
        setSelected(new Set());
        setAnchor(id);
      }
    },
    [rows, anchor],
  );
  const onToggle = useCallback((id: string) => {
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  }, []);
  const onOpen = useCallback((id: string) => connect(id, "default"), [connect]);

  // Keyboard-first: the search box keeps focus while arrows/Enter drive the list.
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      const mod = e.ctrlKey || e.metaKey;
      const el = e.target instanceof Element ? e.target : null;
      // A dialog or overlay has the keyboard (the palette is one too, but Ctrl+K also closes it).
      if (dlg || settings || recordings || (!palette && el?.closest("dialog, [role=dialog], [role=alertdialog]"))) return;
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette((p) => !p);
        return;
      }
      if (palette) return;
      const inSearch = e.target === searchRef.current;
      const typing = isText(e.target) && !inSearch;
      if (typing) return;
      if (mod && e.key.toLowerCase() === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      // A focused control keeps the keys it uses itself (Enter on "Add host" must not open a
      // session); every other shortcut still works after a click on a filter or button.
      const ctl = el?.closest("button, a, select, [role=switch], [role=radio], [role=checkbox]");
      if (ctl && (e.key === "Enter" || e.key === " " || (e.key.startsWith("Arrow") && ctl.matches("select, [role=radio]")))) return;
      switch (e.key) {
        case "ArrowDown":
          e.preventDefault();
          return move(curIdx + 1, e.shiftKey);
        case "ArrowUp":
          e.preventDefault();
          return move(curIdx - 1, e.shiftKey);
        case "PageDown":
          e.preventDefault();
          return move(curIdx + page, e.shiftKey);
        case "PageUp":
          e.preventDefault();
          return move(curIdx - page, e.shiftKey);
        case "Enter":
          // In the search box Enter first searches installed software (again after a passing failure,
          // or when the result shown is older than 5 minutes; the relay keeps answers 60 s).
          if (inSearch && swReady.length && (!swApplied || swRetry || Date.now() - (asOf(swApplied) ?? Date.now()) > 5 * 60_000)) {
            e.preventDefault();
            return finder.enter(q);
          }
          if (!cur) return;
          e.preventDefault();
          return connect(cur, e.shiftKey ? "file-transfer" : e.altKey ? "view-only" : "default");
        case "Escape":
          if (selected.size) {
            e.preventDefault();
            return setSelected(new Set());
          }
          if (q) {
            e.preventDefault();
            return setQ("");
          }
          return;
      }
      if (inSearch) return;
      if (mod && e.key.toLowerCase() === "a") {
        e.preventDefault();
        return setSelected(new Set(rows.map((h) => h.id)));
      }
      if (mod || e.altKey) return;
      switch (e.key) {
        case "Home":
          e.preventDefault();
          return move(0, e.shiftKey);
        case "End":
          e.preventDefault();
          return move(rows.length - 1, e.shiftKey);
        case " ":
          e.preventDefault();
          return cur && onToggle(cur);
        case "/":
          e.preventDefault();
          return searchRef.current?.focus();
        case "?":
          e.preventDefault();
          return setDlg("help");
        case "t":
        case "T":
          e.preventDefault();
          if (targets.length > 1) return setDlg("tags");
          return document.getElementById("detail-tag")?.focus();
        case "p":
        case "P":
          e.preventDefault();
          return targets.length && actions.setPinned(targets, !byId.get(targets[0])?.pinned);
        case "n":
        case "N":
          e.preventDefault();
          return document.getElementById("host-notes")?.focus();
        case "e":
        case "E":
          e.preventDefault();
          return document.getElementById("host-name")?.focus();
        case "Delete":
          e.preventDefault();
          return targets.length && setDlg("remove");
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  const commands: Cmd[] = [
    { id: "search", title: "Search hosts", keys: "/", icon: <Search size={14} />, run: () => searchRef.current?.focus() },
    { id: "add", title: "Add host", icon: <Plus size={14} />, run: () => setDlg("add") },
    { id: "tags", title: targets.length > 1 ? `Edit tags of ${targets.length} selected hosts` : "Edit tags", keys: "T", icon: <Tag size={14} />, run: () => (targets.length > 1 ? setDlg("tags") : document.getElementById("detail-tag")?.focus()) },
    { id: "pin", title: "Pin / unpin", keys: "P", icon: <Pin size={14} />, run: () => targets.length && actions.setPinned(targets, !byId.get(targets[0])?.pinned) },
    { id: "files", title: "Transfer files to the current host", keys: "⇧ Enter", icon: <FolderSync size={14} />, run: () => cur && connect(cur, "file-transfer") },
    { id: "view", title: "View only (current host)", keys: "Alt Enter", icon: <Eye size={14} />, run: () => cur && connect(cur, "view-only") },
    { id: "sessions", title: "Open sessions", hint: "Ctrl+Alt+Del, lock, restart", icon: <Monitor size={14} />, run: () => setDlg("sessions") },
    { id: "save", title: "Save this search", hint: q, icon: <Star size={14} />, run: () => (ro ? toast.show("Saved searches are managed by your team.") : q.trim() && setDlg("save")) },
    { id: "rules", title: "Tag rules", hint: "auto-tag by OS, hostname…", icon: <Wand2 size={14} />, run: () => setDlg("rules") },
    { id: "colors", title: "Tag colours", icon: <PaletteIcon size={14} />, run: () => setDlg("colors") },
    { id: "import", title: "Import hosts", hint: "JSON or CSV", icon: <Upload size={14} />, run: () => setDlg("import") },
    { id: "export", title: "Export hosts", hint: "JSON or CSV", icon: <Download size={14} />, run: () => setDlg("export") },
    { id: "team", title: "Team sync and organisation devices", icon: <Users size={14} />, run: () => setDlg("team") },
    { id: "online", title: "Refresh online status", icon: <Circle size={14} />, run: () => void actions.refreshOnline() },
    { id: "selall", title: "Select all shown", keys: "Ctrl A", run: () => setSelected(new Set(rows.map((h) => h.id))) },
    { id: "help", title: "Keyboard shortcuts and search syntax", keys: "?", icon: <HelpCircle size={14} />, run: () => setDlg("help") },
    ...st.store.searches.map((s) => ({ id: `s${s.id}`, title: `Saved search: ${s.name}`, hint: s.query, icon: <Star size={14} />, run: () => openQuery(s.query) })),
  ];

  const lib: [string, string, number, React.ReactNode][] = [
    ["All hosts", "", hosts.length, <LayoutList size={15} />],
    ["Pinned", "is:pinned", counts.pinned, <Pin size={15} />],
    ["Online", "is:online", counts.online, <Circle size={15} className="text-ok" />],
    ["Recent", "is:recent", counts.recent, <Clock size={15} />],
    ...(st.org.length ? ([["Organisation", "is:org", counts.org, <Users size={15} />]] as [string, string, number, React.ReactNode][]) : []),
    ["Untagged", "-has:tags", counts.untagged, <Tag size={15} />],
    // Built in, not a saved search: saved searches sync to the whole team (spec 10.1).
    ...(hasHealth ? ([["Needs attention", "health:crit | health:stale | health:conflict", counts.attention, <HeartPulse size={15} />]] as [string, string, number, React.ReactNode][]) : []),
  ];

  const navItem = (key: string, text: React.ReactNode, n: number | undefined, active: boolean, onClick: (e: React.MouseEvent) => void, icon?: React.ReactNode, extra?: React.ReactNode) => (
    <div key={key} className="group relative">
      <button
        onClick={onClick}
        aria-current={active || undefined}
        className={`flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-caption ${active ? SELECTED : "text-ink-80 hover:bg-tile-2"}`}
      >
        {icon && <span className={`grid w-4 shrink-0 place-items-center ${active ? "" : "text-muted"}`}>{icon}</span>}
        <span className="min-w-0 flex-1 truncate">{text}</span>
        {n !== undefined && <span className={`text-footnote font-normal tabular-nums ${active ? "" : "text-muted"}`}>{n.toLocaleString()}</span>}
      </button>
      {extra}
    </div>
  );

  const tagNode = (k: string, depth: number): React.ReactNode => {
    const kids = tree.children(k);
    const term = `#${k}`;
    const on = q.split(/\s+/).includes(term);
    const neg = q.split(/\s+/).includes(`-${term}`);
    return (
      <div key={k}>
        <div className="flex items-center" style={{ paddingLeft: depth * 14 }}>
          {kids.length ? (
            <button aria-label={open[k] ? `Collapse ${k}` : `Expand ${k}`} className="grid h-8 w-4 shrink-0 place-items-center text-muted" onClick={() => setOpen({ ...open, [k]: !open[k] })}>
              {open[k] ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            </button>
          ) : (
            <span className="w-4 shrink-0" />
          )}
          <button
            title={`${term}: click to filter, Alt+click to exclude`}
            onClick={(e) => {
              const t = e.altKey || e.shiftKey ? `-${term}` : term;
              const parts = q.split(/\s+/).filter((p) => p && p !== term && p !== `-${term}`);
              setQ((q.split(/\s+/).includes(t) ? parts : [...parts, t]).join(" "));
            }}
            className={`flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md px-1.5 text-left text-caption ${on ? SELECTED : neg ? "text-muted line-through" : "text-ink-80 hover:bg-tile-2"}`}
          >
            <TagDot tag={k} meta={st.store.tags} />
            <span className="min-w-0 flex-1 truncate">{k.slice(k.lastIndexOf(":") + 1)}</span>
            <span className={`text-footnote font-normal tabular-nums ${on ? "" : "text-muted"}`}>{tree.count.get(k)?.toLocaleString()}</span>
          </button>
        </div>
        {open[k] && kids.map((c) => tagNode(c, depth + 1))}
      </div>
    );
  };

  const onlineNote =
    st.onlineState === "unsupported" ? "Online status needs a relay update" : st.onlineState === "error" ? "Online status unavailable" : st.onlineState === "loading" ? "Checking online status…" : null;

  return (
    <div className="flex h-full flex-col overflow-hidden bg-canvas">
      <TitleBar>
        <Logo size={20} />
        <span className="text-caption font-semibold">Hosts</span>
        <div className="flex-1" data-tauri-drag-region />
        <button
          onClick={() => setPalette(true)}
          className="no-drag flex h-8 w-[300px] items-center gap-2 rounded-full border border-hairline bg-parchment px-3.5 text-caption text-muted hover:text-ink"
          aria-label="Open command palette"
        >
          <Search size={14} /> Go to host or command
          <span className="flex-1" />
          <Kbd>Ctrl</Kbd>
          <Kbd>K</Kbd>
        </button>
        <div className="flex-1" data-tauri-drag-region />
        <IconButton label="Open sessions" className="no-drag" onClick={() => setDlg("sessions")}>
          <Monitor size={17} />
        </IconButton>
        <IconButton label="Team" className="no-drag" active={!!st.store.team?.enabled} onClick={() => setDlg("team")}>
          <Users size={17} />
        </IconButton>
        <IconButton label="Keyboard shortcuts" className="no-drag" onClick={() => setDlg("help")}>
          <HelpCircle size={17} />
        </IconButton>
        {!recordingsHidden(eff, rec.st) && (
          <IconButton label="Recordings" className="no-drag" onClick={() => setRecordings(true)}>
            <Video size={17} />
          </IconButton>
        )}
        <IconButton label="Settings" className="no-drag" onClick={() => setSettings(true)}>
          <SettingsIcon size={17} />
        </IconButton>
        {mode.policy !== "forced" && (
          <Button size="sm" variant="ghost" className="no-drag" onClick={() => mode.set("simple")} title="Back to the simple home view (your ID and recent devices)">
            Simple view
          </Button>
        )}
      </TitleBar>

      <div className="flex min-h-0 flex-1">
        <nav aria-label="Library" className="flex w-[232px] shrink-0 flex-col overflow-y-auto border-r border-hairline bg-parchment/60 px-3 pb-4">
          <div className="space-y-0.5 pt-1">{lib.map(([t, query, n, icon]) => navItem(t, t, n, q === query, () => setQ(query), icon))}</div>

          <div className="mt-5 flex items-center justify-between px-2 text-footnote font-semibold text-muted">
            Saved searches
            <IconButton label={ro ? "Saved searches are managed by your team" : "Save this search"} className="!h-6 !w-6" disabled={ro || !q.trim()} onClick={() => setDlg("save")}>
              <Plus size={13} />
            </IconButton>
          </div>
          <div className="mt-1 space-y-0.5">
            {st.store.searches.length === 0 && <div className="px-2 text-footnote text-muted">{ro ? "Managed by your team." : "Search, then + to keep it here."}</div>}
            {st.store.searches.map((s) =>
              navItem(
                s.id,
                s.name,
                searchCounts.get(s.id),
                q === s.query,
                () => openQuery(s.query),
                <Star size={14} />,
                !ro && <button
                  aria-label={`Delete saved search ${s.name}`}
                  onClick={() => actions.setStore((x) => ({ ...x, searches: x.searches.filter((y) => y.id !== s.id) }))}
                  className="absolute right-1 top-1 hidden h-6 w-6 place-items-center rounded-full bg-parchment text-muted hover:text-danger group-hover:grid"
                >
                  <X size={12} />
                </button>,
              ),
            )}
          </div>

          <div className="mt-5 flex items-center justify-between px-2 text-footnote font-semibold text-muted">
            Tags
            <span className="flex">
              <IconButton label="Tag rules" className="!h-6 !w-6" onClick={() => setDlg("rules")}>
                <Wand2 size={13} />
              </IconButton>
              <IconButton label="Tag colours" className="!h-6 !w-6" onClick={() => setDlg("colors")}>
                <PaletteIcon size={13} />
              </IconButton>
            </span>
          </div>
          <div className="mt-1" aria-label="Tags">
            {tree.roots.length === 0 && <div className="px-2 text-footnote text-muted">Select hosts and press T to tag them.</div>}
            {tree.roots.map((r) => tagNode(r, 0))}
          </div>
        </nav>

        <main className="flex min-w-0 flex-1 flex-col">
          <div className="flex shrink-0 items-center gap-3 px-4 pb-2 pt-3">
            <label className="flex h-10 min-w-0 flex-1 items-center gap-2.5 rounded-full border border-hairline bg-parchment px-4 focus-within:border-focus">
              <Search size={16} className="shrink-0 text-muted" />
              <input
                ref={searchRef}
                autoFocus
                value={q}
                onChange={(e) => setQ(e.target.value.slice(0, 1000))}
                placeholder="Search hosts…"
                title="Search name, ID, #tag, os:mac, is:online, seen:<7d"
                aria-label="Search hosts"
                spellCheck={false}
                autoComplete="off"
                className="min-w-0 flex-1 bg-transparent font-mono text-caption text-ink outline-none placeholder:font-sans placeholder:text-faint focus-visible:outline-none"
              />
              {parsed.error && <span className="shrink-0 text-footnote text-warn">{parsed.error}</span>}
              {q && (
                <IconButton label="Clear search" className="!h-6 !w-6" onClick={() => setQ("")}>
                  <X size={13} />
                </IconButton>
              )}
            </label>
            <Button size="md" variant="pearl" onClick={() => setDlg("add")}>
              <Plus size={15} /> Add host
            </Button>
          </div>

          {/* Always in the page (empty: hidden), so a screen reader announces each change. */}
          <div role="status" data-testid="search-notes" className="mx-4 mb-2 shrink-0 rounded-xl bg-parchment px-3 py-2 text-caption text-ink empty:hidden">
            {swLines.map((l) => (
              <div key={l}>{l}</div>
            ))}
            {checkHint && <div>{checkHint}</div>}
            {swTerms.length > 0 && <div className="text-footnote text-muted">Each software search is sent to the servers and recorded in the team's audit log with its text.</div>}
          </div>
          {st.saveError && (
            <div role="alert" className="mx-4 mb-2 shrink-0 rounded-xl bg-danger/10 px-3 py-2 text-caption text-danger">
              {st.saveError}
            </div>
          )}
          {sel.length > 0 && (
            <div role="toolbar" aria-label="Selection" className="animate-fade mx-4 mb-2 flex min-h-10 shrink-0 flex-wrap items-center gap-x-2 rounded-xl py-1 bg-accent/12 px-3 text-caption">
              <span className="font-semibold" data-testid="selection-count">
                {sel.length.toLocaleString()} selected
              </span>
              <span className="flex-1" />
              <Button size="sm" variant="ghost" onClick={() => setDlg("tags")}>
                <Tag size={14} /> Tags… <Kbd>T</Kbd>
              </Button>
              <Button size="sm" variant="ghost" onClick={() => actions.setPinned(sel, !sel.every((id) => byId.get(id)?.pinned))}>
                <Pin size={14} /> {sel.every((id) => byId.get(id)?.pinned) ? "Unpin" : "Pin"}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setDlg("export")}>
                <Download size={14} /> Export
              </Button>
              <Button size="sm" variant="ghost" onClick={() => navigator.clipboard.writeText(sel.join("\n")).then(() => toast.show(`${sel.length} IDs copied`), () => {})}>
                Copy IDs
              </Button>
              <Button size="sm" variant="ghost" className="!text-danger" onClick={() => setDlg("remove")}>
                <Trash2 size={14} /> Remove
              </Button>
              <IconButton label="Clear selection" className="!h-7 !w-7" onClick={() => setSelected(new Set())}>
                <X size={14} />
              </IconButton>
            </div>
          )}

          {!st.loaded ? (
            <div className="grid flex-1 place-items-center text-caption text-muted">Loading hosts…</div>
          ) : hosts.length === 0 ? (
            <div className="grid flex-1 place-items-center">
              <div className="text-center">
                <div className="text-callout font-semibold">No hosts yet</div>
                <div className="mt-1 text-caption text-muted">Devices you connect to appear here. Add or import your list, or turn on Team to see your organisation's devices.</div>
                <div className="mt-4 flex justify-center gap-2">
                  <Button variant="primary" onClick={() => setDlg("add")}>
                    Add host
                  </Button>
                  <Button variant="pearl" onClick={() => setDlg("import")}>
                    Import…
                  </Button>
                  <Button variant="pearl" onClick={() => setDlg("team")}>
                    Team…
                  </Button>
                </div>
              </div>
            </div>
          ) : rows.length === 0 ? (
            <div className="grid flex-1 place-items-center text-center">
              <div>
                <div className="text-callout font-semibold">No host matches</div>
                <div className="mt-1 text-caption text-muted">
                  Press <Kbd>?</Kbd> for the search syntax.
                </div>
              </div>
            </div>
          ) : (
            <HostTable rows={rows} cursor={cur} selected={selected} meta={st.store.tags} sort={sort} onSort={setSort} onRow={onRow} onToggle={onToggle} onToggleAll={() => setSelected(rows.every((h) => selected.has(h.id)) ? new Set() : new Set(rows.map((h) => h.id)))} onOpen={onOpen} health={hasHealth} />
          )}

          <footer className="flex h-8 shrink-0 items-center gap-3 whitespace-nowrap border-t border-hairline px-4 text-footnote text-muted" data-testid="status-bar">
            <span data-testid="result-count">
              {rows.length === hosts.length ? `${hosts.length.toLocaleString()} hosts` : `${rows.length.toLocaleString()} of ${hosts.length.toLocaleString()} hosts`}
            </span>
            {st.onlineState === "ok" && <span>{counts.online.toLocaleString()} online</span>}
            {onlineNote && <span>{onlineNote}</span>}
            {st.store.team?.enabled && st.team.error && <span className="text-warn">Team: {st.team.error}</span>}
            {st.store.team?.enabled && st.statusError && <span className={st.statusError === "this relay does not report health" ? "" : "text-warn"}>Health: {st.statusError}</span>}
            <span className="flex-1" />
            <span className="hidden tabular-nums text-faint xl:inline" data-testid="search-ms">
              search {perf.last.toFixed(1)} ms
            </span>
            {!canConnect && <span className="text-warn">Connecting is turned off by your organisation</span>}
            {status?.id && (
              <button className="min-w-0 truncate font-mono tabular-nums text-muted hover:text-ink" title="This device's ID: click to copy" onClick={() => navigator.clipboard.writeText(status.id).then(() => toast.show("Your ID copied"), () => {})}>
                This device {formatId(status.id)}
              </button>
            )}
          </footer>
        </main>

        {curHost && <Detail key={curHost.id} h={curHost} meta={st.store.tags} known={known} canConnect={canConnect} onConnect={connect} onToast={toast.show} onRemove={() => setDlg("remove")} nameOf={nameOf} holderOf={holderOf} />}
      </div>

      {palette && <Palette hosts={hosts} commands={commands} onClose={() => setPalette(false)} onHost={connect} />}
      {dlg === "tags" && <BulkTags hosts={targets.map((id) => byId.get(id)!).filter(Boolean)} meta={st.store.tags} known={known} onClose={() => setDlg(null)} />}
      {dlg === "rules" && <Rules hosts={hosts} store={st.store} onClose={() => setDlg(null)} />}
      {dlg === "colors" && <TagColors tags={known} meta={st.store.tags} onClose={() => setDlg(null)} />}
      {dlg === "export" && <Export store={st.store} selection={sel} shown={rows} hosts={byId} health={hasHealth} onClose={() => setDlg(null)} onToast={toast.show} />}
      {dlg === "import" && <Import store={st.store} onClose={() => setDlg(null)} onToast={toast.show} />}
      {dlg === "team" && <Team onClose={() => setDlg(null)} onToast={toast.show} />}
      {dlg === "add" && <AddHost known={known} onClose={() => setDlg(null)} onAdded={(id) => (setQ(""), setCursor(id))} />}
      {dlg === "save" && <SaveSearch query={q.trim()} onClose={() => setDlg(null)} />}
      {dlg === "sessions" && <Sessions hosts={byId} onClose={() => setDlg(null)} onToast={toast.show} />}
      {dlg === "help" && <Help onClose={() => setDlg(null)} />}
      {dlg === "remove" && (
        <Modal onClose={() => setDlg(null)}>
          <h2 className="text-headline font-semibold">Remove {targets.length === 1 ? label(byId.get(targets[0]) ?? { id: targets[0], name: "", hostname: "", user: "" }) : `${targets.length.toLocaleString()} hosts`}?</h2>
          <p className="mt-2 text-caption text-muted">Removes names, tags, notes and the recent entry on this PC. Organisation devices stay listed while Team shows them.</p>
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="pearl" onClick={() => setDlg(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              data-autofocus
              onClick={() => {
                void actions.remove(targets);
                setSelected(new Set());
                setDlg(null);
              }}
            >
              <Trash2 size={14} /> Remove
            </Button>
          </div>
        </Modal>
      )}
      {recordings && <Recordings onClose={() => setRecordings(false)} onToast={toast.show} />}
      {settings && <Settings onClose={() => setSettings(false)} status={status} />}
      <RecordingSaved />
      <UpdateCard />
      {toast.node}
    </div>
  );
}
