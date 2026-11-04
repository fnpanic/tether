import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowDownToLine, ArrowLeft, ArrowUpFromLine, CheckCircle2, File, Folder, FolderPlus, HardDrive, RefreshCw, Trash2, XCircle } from "lucide-react";
import { formatId, invoke, subscribe, videoChannel } from "../lib/ipc";
import { Button, IconButton, Logo, TitleBar } from "../components/ui";

interface Entry {
  name: string;
  type: number; // 0 dir-link? 1 dir, 2 drive, 3 file-link, 4 file (core FileType)
  size: number;
  modified: number;
  is_hidden?: boolean;
}

const isDir = (e: Entry) => e.type === 0 || e.type === 1 || e.type === 2;

interface Job {
  id: number;
  name: string;
  upload: boolean;
  total: number;
  done: number;
  speed: number;
  state: "running" | "done" | "error";
  err?: string;
}

function fmtSize(n: number) {
  if (!n) return "—";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(i ? 1 : 0)} ${u[i]}`;
}

function join(dir: string, name: string, sep: string) {
  if (!dir) return name;
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

function parent(dir: string, sep: string) {
  const t = dir.endsWith(sep) && dir.length > 1 ? dir.slice(0, -1) : dir;
  const i = t.lastIndexOf(sep);
  if (i <= 0) return sep === "/" ? "/" : t.length <= 3 ? "" : t.slice(0, i + 1);
  return t.slice(0, i) || sep;
}

let nextJob = 1;

export function FileTransfer({ sessionKey: key, peer }: { sessionKey: string; peer: string }) {
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState<null | { wrong: boolean }>(null);
  const [pw, setPw] = useState("");
  const [waiting, setWaiting] = useState(false);
  const [remoteSep, setRemoteSep] = useState("/");
  const localSep = navigator.userAgent.includes("Windows") ? "\\" : "/";
  const [local, setLocal] = useState<{ path: string; entries: Entry[] }>({ path: "", entries: [] });
  const [remote, setRemote] = useState<{ path: string; entries: Entry[] }>({ path: "", entries: [] });
  const [sel, setSel] = useState<{ side: "local" | "remote"; name: string } | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [hostname, setHostname] = useState("");
  const remotePathRef = useRef("");

  const loadLocal = useCallback(async (path: string) => {
    try {
      const r = await invoke<{ path: string; entries: Entry[] }>("fs_local_dir", { path, hidden: false });
      setLocal({ path: r.path || path, entries: sortEntries(r.entries) });
    } catch (e) {
      setError(String(e));
    }
  }, []);
  const loadRemote = useCallback(
    (path: string) => {
      remotePathRef.current = path;
      invoke("fs_remote_dir", { key, path, hidden: false }).catch((e) => setError(String(e)));
    },
    [key],
  );

  useEffect(() => {
    let alive = true;
    const sub = subscribe("session", (e) => {
      const p = e.payload;
      switch (e.name) {
        case "peer_info": {
          // The host lists its home folder by itself right after login (`folder_files`). A
          // password prompt the core showed while the customer could still click Accept is over.
          setPassword(null);
          setConnected(true);
          setHostname(p.hostname);
          const win = (p.platform ?? "").toLowerCase().includes("win");
          setRemoteSep(win ? "\\" : "/");
          break;
        }
        case "folder_files":
          if (!p.is_local && !p.only_count) setRemote({ path: p.path, entries: sortEntries(p.entries ?? []) });
          break;
        case "job_progress":
          setJobs((js) => js.map((j) => (j.id === p.id ? { ...j, done: p.finished_size, speed: p.speed } : j)));
          break;
        case "job_done":
          setJobs((js) => js.map((j) => (j.id === p.id ? { ...j, state: "done", done: j.total || j.done } : j)));
          loadRemote(remotePathRef.current);
          setLocal((l) => (loadLocal(l.path), l));
          break;
        case "job_error":
          setJobs((js) => js.map((j) => (j.id === p.id ? { ...j, state: "error", err: p.err } : j)));
          break;
        case "override_file":
          invoke("fs_override", { key, id: p.id, fileNum: p.file_num, needOverride: true, remember: true, isUpload: p.is_upload }).catch((e) => setError(String(e)));
          break;
        case "msgbox":
          if (p.type === "input-password") setPassword({ wrong: false });
          else if (p.type === "re-input-password") setPassword({ wrong: true });
          else if (p.type.startsWith("wait")) setWaiting(true);
          else if (p.type === "error") setError(p.text);
          break;
      }
    });
    (async () => {
      loadLocal(await invoke<string>("fs_home_dir"));
      await sub.ready;
      // StrictMode's discarded first run must not start (and consume the pending login of) a session.
      if (!alive) return;
      await invoke("session_start", { key, peer, conn: "file-transfer", webcodecs: [], video: videoChannel(() => {}) });
    })().catch((e) => alive && setError(String(e)));
    return () => {
      alive = false;
      sub.off();
      invoke("session_close", { key });
    };
  }, [key, peer, loadLocal, loadRemote]);

  const transfer = (side: "local" | "remote", e: Entry) => {
    const upload = side === "local";
    const id = nextJob++;
    const from = join(upload ? local.path : remote.path, e.name, upload ? localSep : remoteSep);
    const to = join(upload ? remote.path : local.path, e.name, upload ? remoteSep : localSep);
    setJobs((js) => [{ id, name: e.name, upload, total: e.size, done: 0, speed: 0, state: "running" }, ...js]);
    invoke("fs_transfer", { key, id, path: from, to, fileNum: 0, hidden: false, isRemote: !upload }).catch((err) =>
      setJobs((js) => js.map((j) => (j.id === id ? { ...j, state: "error", err: String(err) } : j))),
    );
  };

  if (password) {
    return (
      <Shell title={formatId(peer)}>
        <form
          className="m-auto w-[340px]"
          onSubmit={(e) => {
            e.preventDefault();
            invoke("session_login", { key, password: pw, remember: true }).catch((e) => setError(String(e)));
            setPassword(null);
            setPw("");
          }}
        >
          <div className="text-callout font-semibold">Password required</div>
          <div className={`mt-1 text-caption ${password.wrong ? "text-danger" : "text-muted"}`}>{password.wrong ? "Wrong password, try again" : "Enter the one-time password shown on the remote device"}</div>
          <div className="mt-1 text-footnote text-muted">Or ask the person there to click Accept: this window then continues by itself.</div>
          <input type="password" autoFocus value={pw} onChange={(e) => setPw(e.target.value)} className="mt-3 h-10 w-full rounded-xl border border-hairline bg-parchment px-3.5 text-caption outline-none focus-visible:border-focus" />
          <Button type="submit" variant="primary" className="mt-3 w-full" disabled={!pw}>
            Connect
          </Button>
        </form>
      </Shell>
    );
  }

  return (
    <Shell title={hostname ? `${hostname} · ${formatId(peer)}` : formatId(peer)}>
      {error && (
        <div className="mx-4 mb-2 flex items-center gap-2 rounded-xl bg-danger/10 px-3 py-2 text-caption text-danger">
          <XCircle size={15} /> {error}
          <button className="ml-auto text-footnote underline" onClick={() => setError(null)}>
            Dismiss
          </button>
        </div>
      )}
      <div className="grid min-h-0 flex-1 grid-cols-2 gap-3 px-4">
        <Pane
          title="This computer"
          icon={<HardDrive size={15} />}
          path={local.path}
          entries={local.entries}
          selected={sel?.side === "local" ? sel.name : null}
          onSelect={(n) => setSel({ side: "local", name: n })}
          onOpen={(e) => loadLocal(join(local.path, e.name, localSep))}
          onUp={() => loadLocal(parent(local.path, localSep))}
          onRefresh={() => loadLocal(local.path)}
          onTransfer={(e) => transfer("local", e)}
          transferIcon={<ArrowUpFromLine size={15} />}
          transferLabel="Upload"
          onDropFrom="remote"
          onDrop={(name) => {
            const e = remote.entries.find((x) => x.name === name);
            if (e) transfer("remote", e);
          }}
        />
        <Pane
          title={hostname || "Remote"}
          icon={<HardDrive size={15} />}
          path={remote.path}
          entries={remote.entries}
          loading={!connected}
          loadingText={waiting ? "Waiting for the person at the remote computer to click Accept…" : "Connecting…"}
          selected={sel?.side === "remote" ? sel.name : null}
          onSelect={(n) => setSel({ side: "remote", name: n })}
          onOpen={(e) => loadRemote(join(remote.path, e.name, remoteSep))}
          onUp={() => loadRemote(parent(remote.path, remoteSep))}
          onRefresh={() => loadRemote(remote.path)}
          onTransfer={(e) => transfer("remote", e)}
          transferIcon={<ArrowDownToLine size={15} />}
          transferLabel="Download"
          onNewFolder={() => {
            const n = prompt("Folder name");
            if (n) invoke("fs_create_dir", { key, id: nextJob++, path: join(remote.path, n, remoteSep), isRemote: true }).then(() => setTimeout(() => loadRemote(remote.path), 300));
          }}
          onDelete={(e) => {
            if (confirm(`Delete "${e.name}" on the remote device?`)) {
              invoke("fs_remove", { key, id: nextJob++, path: join(remote.path, e.name, remoteSep), isRemote: true, isDir: isDir(e) }).then(() => setTimeout(() => loadRemote(remote.path), 300));
            }
          }}
          onDropFrom="local"
          onDrop={(name) => {
            const e = local.entries.find((x) => x.name === name);
            if (e) transfer("local", e);
          }}
        />
      </div>
      <Jobs jobs={jobs} onCancel={(id) => invoke("fs_cancel", { key, id }).catch((e) => setError(String(e)))} onClear={() => setJobs((js) => js.filter((j) => j.state === "running"))} />
    </Shell>
  );
}

function sortEntries(e: Entry[]) {
  return [...e].sort((a, b) => Number(isDir(b)) - Number(isDir(a)) || a.name.localeCompare(b.name, undefined, { numeric: true }));
}

function Shell({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="flex h-full flex-col">
      <TitleBar>
        <Logo size={20} />
        <span className="text-caption font-semibold">File transfer</span>
        <span className="truncate text-caption text-muted">{title}</span>
      </TitleBar>
      {children}
    </div>
  );
}

interface PaneProps {
  title: string;
  icon: React.ReactNode;
  path: string;
  entries: Entry[];
  loading?: boolean;
  loadingText?: string;
  selected: string | null;
  onSelect: (n: string) => void;
  onOpen: (e: Entry) => void;
  onUp: () => void;
  onRefresh: () => void;
  onTransfer: (e: Entry) => void;
  transferIcon: React.ReactNode;
  transferLabel: string;
  onNewFolder?: () => void;
  onDelete?: (e: Entry) => void;
  onDropFrom: "local" | "remote";
  onDrop: (name: string) => void;
}

function Pane(p: PaneProps) {
  const [over, setOver] = useState(false);
  const sel = p.entries.find((e) => e.name === p.selected);
  return (
    <div
      className={`flex min-h-0 flex-col overflow-hidden rounded-2xl border bg-tile transition ${over ? "border-accent ring-2 ring-accent/30" : "border-hairline"}`}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes(`x-tether/${p.onDropFrom}`)) {
          e.preventDefault();
          setOver(true);
        }
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        setOver(false);
        const n = e.dataTransfer.getData(`x-tether/${p.onDropFrom}`);
        if (n) p.onDrop(n);
      }}
    >
      <div className="flex items-center gap-2 border-b border-hairline px-3 py-2">
        <span className="text-muted">{p.icon}</span>
        <span className="text-caption font-semibold">{p.title}</span>
        <div className="flex-1" />
        {p.onNewFolder && (
          <IconButton label="New folder" onClick={p.onNewFolder}>
            <FolderPlus size={15} />
          </IconButton>
        )}
        {p.onDelete && sel && (
          <IconButton label="Delete" onClick={() => p.onDelete!(sel)}>
            <Trash2 size={15} />
          </IconButton>
        )}
        <IconButton label="Refresh" onClick={p.onRefresh}>
          <RefreshCw size={15} />
        </IconButton>
        <Button size="sm" variant="primary" disabled={!sel} onClick={() => sel && p.onTransfer(sel)}>
          {p.transferIcon} {p.transferLabel}
        </Button>
      </div>
      <div className="flex items-center gap-1 border-b border-hairline px-2 py-1.5">
        <IconButton label="Up" onClick={p.onUp}>
          <ArrowLeft size={15} />
        </IconButton>
        <div className="truncate font-mono text-footnote text-muted" title={p.path}>
          {p.path || "Drives"}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {p.loading ? (
          <div className="grid h-full place-items-center px-4 text-center text-caption text-muted">{p.loadingText ?? "Connecting…"}</div>
        ) : p.entries.length === 0 ? (
          <div className="grid h-full place-items-center text-caption text-muted">Empty folder</div>
        ) : (
          p.entries.map((e) => (
            <div
              key={e.name}
              draggable
              onDragStart={(ev) => ev.dataTransfer.setData(`x-tether/${p.onDropFrom === "local" ? "remote" : "local"}`, e.name)}
              onClick={() => p.onSelect(e.name)}
              onDoubleClick={() => (isDir(e) ? p.onOpen(e) : p.onTransfer(e))}
              className={`mx-1 flex cursor-default items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-caption ${p.selected === e.name ? "bg-accent/15" : "hover:bg-parchment"}`}
            >
              {isDir(e) ? <Folder size={16} className="shrink-0 text-accent" /> : <File size={16} className="shrink-0 text-muted" />}
              <span className="min-w-0 flex-1 truncate">{e.name}</span>
              <span className="w-20 shrink-0 text-right font-mono text-footnote text-muted">{isDir(e) ? "" : fmtSize(e.size)}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function Jobs({ jobs, onCancel, onClear }: { jobs: Job[]; onCancel: (id: number) => void; onClear: () => void }) {
  if (!jobs.length) return <div className="h-4" />;
  return (
    <div className="m-4 mt-3 max-h-40 overflow-y-auto rounded-2xl border border-hairline bg-tile">
      <div className="flex items-center justify-between px-3 py-2">
        <span className="text-caption font-semibold">Transfers</span>
        <button className="text-footnote text-muted hover:text-ink" onClick={onClear}>
          Clear finished
        </button>
      </div>
      {jobs.map((j) => {
        const pct = j.total ? Math.min(100, (j.done / j.total) * 100) : j.state === "done" ? 100 : 0;
        return (
          <div key={j.id} className="flex items-center gap-3 border-t border-hairline px-3 py-2 text-caption">
            {j.upload ? <ArrowUpFromLine size={14} className="text-muted" /> : <ArrowDownToLine size={14} className="text-muted" />}
            <span className="w-48 truncate">{j.name}</span>
            <div className="h-1.5 flex-1 overflow-hidden rounded-full bg-parchment">
              <div className={`h-full rounded-full transition-all ${j.state === "error" ? "bg-danger" : "bg-accent"}`} style={{ width: `${pct}%` }} />
            </div>
            <span className="w-24 text-right font-mono text-footnote text-muted">
              {j.state === "running" ? `${fmtSize(j.speed)}/s` : j.state === "done" ? <CheckCircle2 size={15} className="ml-auto text-ok" /> : <span className="text-danger">{j.err ?? "failed"}</span>}
            </span>
            {j.state === "running" && (
              <button aria-label="Cancel" className="text-muted hover:text-danger" onClick={() => onCancel(j.id)}>
                <XCircle size={15} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
