import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowDownToLine, ArrowLeft, ArrowUpFromLine, CheckCircle2, File, Folder, FolderPlus, HardDrive, RefreshCw, Trash2, XCircle } from "lucide-react";
import { formatId, invoke, subscribe, videoChannel } from "../lib/ipc";
import { Button, IconButton, Logo, Modal, TitleBar } from "../components/ui";
import { clickSelect, emptySelection, moveSelect, selectAll } from "../lib/selection";
import { passwordWhy, RESUME_GRACE_MS } from "../lib/login";

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
  from: string;
  to: string;
  total: number;
  done: number;
  speed: number;
  state: "queued" | "running" | "done" | "error";
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
// Jobs run side by side in the core (it interleaves them); a few at a time keeps each one moving.
const MAX_RUNNING = 2;

export function FileTransfer({ sessionKey: key, peer }: { sessionKey: string; peer: string }) {
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState<null | { wrong: boolean; why: string | null }>(null);
  // A password typed in this window (only then is a rejection "wrong"), and the delayed screen.
  const typed = useRef(false);
  const pwTimer = useRef(0);
  const [pw, setPw] = useState("");
  const [waiting, setWaiting] = useState(false);
  const [remoteSep, setRemoteSep] = useState("/");
  const localSep = navigator.userAgent.includes("Windows") ? "\\" : "/";
  const [local, setLocal] = useState<{ path: string; entries: Entry[] }>({ path: "", entries: [] });
  const [remote, setRemote] = useState<{ path: string; entries: Entry[] }>({ path: "", entries: [] });
  const [toDelete, setToDelete] = useState<Entry[] | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [hostname, setHostname] = useState("");
  const remotePathRef = useRef("");
  // Remote folders being deleted: job id -> [folder path, separator].
  const dirDeletes = useRef(new Map<number, [string, string]>());

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
          clearTimeout(pwTimer.current);
          setPassword(null);
          setConnected(true);
          setHostname(p.hostname);
          const win = (p.platform ?? "").toLowerCase().includes("win");
          setRemoteSep(win ? "\\" : "/");
          break;
        }
        case "folder_files":
          // A job's own file list (id = job id) gives its total size; listings come with id 0.
          if (dirDeletes.current.has(p.id)) {
            // The folder's files (names relative to it): remove each, then the emptied folders.
            const [dir, sep] = dirDeletes.current.get(p.id)!;
            dirDeletes.current.delete(p.id);
            const files: Entry[] = p.entries ?? [];
            Promise.all(files.map((f) => invoke("fs_remove", { key, id: p.id, path: join(dir, f.name, sep), isRemote: true, isDir: false })))
              .then(() => invoke("fs_remove_empty_dirs", { key, id: p.id, path: dir, isRemote: true }))
              .catch((e) => setError(String(e)));
          } else if (p.id) setJobs((js) => js.map((j) => (j.id === p.id && p.total_size ? { ...j, total: p.total_size } : j)));
          else if (!p.is_local && !p.only_count) {
            remotePathRef.current = p.path;
            setRemote({ path: p.path, entries: sortEntries(p.entries ?? []) });
          }
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
          if (p.type === "input-password" || p.type === "re-input-password") {
            const why = passwordWhy(p.text, false) ?? (p.type === "re-input-password" && !typed.current ? "The password of your session does not work here any more (a one-time password changes after each login)." : null);
            const show = () => setPassword({ wrong: p.type === "re-input-password" && typed.current, why });
            clearTimeout(pwTimer.current);
            // Opened from a session: the core asks first, then the host lets it join without a password.
            if (p.type === "input-password" && !p.text) pwTimer.current = window.setTimeout(show, RESUME_GRACE_MS);
            else show();
          }
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
      clearTimeout(pwTimer.current);
      invoke("session_close", { key });
    };
  }, [key, peer, loadLocal, loadRemote]);

  const transfer = (side: "local" | "remote", es: Entry[]) => {
    const upload = side === "local";
    const queued = es.map((e): Job => ({
      id: nextJob++,
      name: e.name,
      upload,
      from: join(upload ? local.path : remote.path, e.name, upload ? localSep : remoteSep),
      to: join(upload ? remote.path : local.path, e.name, upload ? remoteSep : localSep),
      total: isDir(e) ? 0 : e.size,
      done: 0,
      speed: 0,
      state: "queued",
    }));
    setJobs((js) => [...queued.reverse(), ...js]);
  };

  // Start queued jobs (oldest first) while fewer than MAX_RUNNING run.
  useEffect(() => {
    const free = MAX_RUNNING - jobs.filter((j) => j.state === "running").length;
    const start = jobs.filter((j) => j.state === "queued").reverse().slice(0, Math.max(0, free));
    if (!start.length) return;
    const ids = new Set(start.map((j) => j.id));
    setJobs((js) => js.map((j) => (ids.has(j.id) ? { ...j, state: "running" } : j)));
    for (const j of start) {
      invoke("fs_transfer", { key, id: j.id, path: j.from, to: j.to, fileNum: 0, hidden: false, isRemote: !j.upload }).catch((err) =>
        setJobs((js) => js.map((x) => (x.id === j.id ? { ...x, state: "error", err: String(err) } : x))),
      );
    }
  }, [jobs, key]);

  const cancel = (j: Job) => {
    // The core reports nothing for a cancelled job: end it here so the queue moves on.
    if (j.state === "running") invoke("fs_cancel", { key, id: j.id }).catch((e) => setError(String(e)));
    setJobs((js) => js.map((x) => (x.id === j.id ? { ...x, state: "error", err: "Cancelled" } : x)));
  };

  const removeRemote = (es: Entry[]) => {
    setToDelete(null);
    Promise.all(
      es.map((e) => {
        const id = nextJob++;
        const path = join(remote.path, e.name, remoteSep);
        if (isDir(e)) dirDeletes.current.set(id, [path, remoteSep]);
        return invoke("fs_remove", { key, id, path, isRemote: true, isDir: isDir(e) });
      }),
    )
      .catch((e) => setError(String(e)))
      .finally(() => setTimeout(() => loadRemote(remote.path), 300));
  };

  if (password) {
    return (
      <Shell title={formatId(peer)}>
        <form
          className="m-auto w-[340px]"
          onSubmit={(e) => {
            e.preventDefault();
            typed.current = true;
            invoke("session_login", { key, password: pw, remember: true }).catch((e) => setError(String(e)));
            setPassword(null);
            setPw("");
          }}
        >
          <div className="text-callout font-semibold">Password required</div>
          <div className={`mt-1 text-caption ${password.wrong ? "text-danger" : "text-muted"}`}>{password.wrong ? "Wrong password, try again" : "Enter the one-time password shown on the remote device"}</div>
          {password.why && <div className="mt-1 text-footnote">{password.why}</div>}
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
          onOpen={(e) => loadLocal(join(local.path, e.name, localSep))}
          onUp={() => loadLocal(parent(local.path, localSep))}
          onRefresh={() => loadLocal(local.path)}
          onTransfer={(es) => transfer("local", es)}
          transferIcon={<ArrowUpFromLine size={15} />}
          transferLabel="Upload"
          onDropFrom="remote"
          onDrop={(names) => transfer("remote", remote.entries.filter((x) => names.includes(x.name)))}
        />
        <Pane
          title={hostname || "Remote"}
          icon={<HardDrive size={15} />}
          path={remote.path}
          entries={remote.entries}
          loading={!connected}
          loadingText={waiting ? "Waiting for the person at the remote computer to click Accept…" : "Connecting…"}
          onOpen={(e) => loadRemote(join(remote.path, e.name, remoteSep))}
          onUp={() => loadRemote(parent(remote.path, remoteSep))}
          onRefresh={() => loadRemote(remote.path)}
          onTransfer={(es) => transfer("remote", es)}
          transferIcon={<ArrowDownToLine size={15} />}
          transferLabel="Download"
          onNewFolder={() => {
            const n = prompt("Folder name");
            if (n) invoke("fs_create_dir", { key, id: nextJob++, path: join(remote.path, n, remoteSep), isRemote: true }).then(() => setTimeout(() => loadRemote(remote.path), 300));
          }}
          onDelete={setToDelete}
          onDropFrom="local"
          onDrop={(names) => transfer("local", local.entries.filter((x) => names.includes(x.name)))}
        />
      </div>
      <Jobs jobs={jobs} onCancel={cancel} onClear={() => setJobs((js) => js.filter((j) => j.state === "running" || j.state === "queued"))} />
      {toDelete && (
        <Modal width={360} onClose={() => setToDelete(null)}>
          <div className="text-headline font-semibold">{toDelete.length === 1 ? `Delete "${toDelete[0].name}"?` : `Delete ${toDelete.length} items?`}</div>
          <p className="mt-2 text-callout text-muted">
            {toDelete.length === 1 ? "It is" : "They are"} deleted on the remote device{toDelete.some(isDir) ? ", folders with everything in them" : ""}. This cannot be undone.
          </p>
          <div className="mt-6 flex gap-2">
            <Button className="flex-1" onClick={() => setToDelete(null)}>
              Cancel
            </Button>
            <Button variant="danger" className="flex-1" autoFocus onClick={() => removeRemote(toDelete)}>
              Delete
            </Button>
          </div>
        </Modal>
      )}
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
  onOpen: (e: Entry) => void;
  onUp: () => void;
  onRefresh: () => void;
  onTransfer: (es: Entry[]) => void;
  transferIcon: React.ReactNode;
  transferLabel: string;
  onNewFolder?: () => void;
  onDelete?: (es: Entry[]) => void;
  onDropFrom: "local" | "remote";
  onDrop: (names: string[]) => void;
}

function Pane(p: PaneProps) {
  const [over, setOver] = useState(false);
  const [sel, setSel] = useState(emptySelection);
  useEffect(() => setSel(emptySelection), [p.path]);
  const names = p.entries.map((e) => e.name);
  const selected = p.entries.filter((e) => sel.names.includes(e.name));
  const n = selected.length;
  const onKey = (ev: React.KeyboardEvent) => {
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "a") setSel(selectAll(names, sel));
    else if (ev.key === "ArrowDown" || ev.key === "ArrowUp") setSel(moveSelect(names, sel, ev.key === "ArrowDown" ? 1 : -1, ev.shiftKey));
    else if (ev.key === "Enter" && n) n === 1 && isDir(selected[0]) ? p.onOpen(selected[0]) : p.onTransfer(selected);
    else if (ev.key === "Delete" && n && p.onDelete) p.onDelete(selected);
    else if (ev.key === "Escape" && n) setSel(emptySelection);
    else return;
    ev.preventDefault();
    ev.stopPropagation();
  };
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
        const d = e.dataTransfer.getData(`x-tether/${p.onDropFrom}`);
        if (d) p.onDrop(JSON.parse(d));
      }}
    >
      <div className="flex items-center gap-2 border-b border-hairline px-3 py-2">
        <span className="shrink-0 text-muted">{p.icon}</span>
        <span className="min-w-0 truncate text-caption font-semibold" title={p.title}>{p.title}</span>
        <div className="flex-1" />
        {p.onNewFolder && (
          <IconButton label="New folder" onClick={p.onNewFolder}>
            <FolderPlus size={15} />
          </IconButton>
        )}
        {p.onDelete && n > 0 && (
          <IconButton label={n > 1 ? `Delete ${n} items` : "Delete"} onClick={() => p.onDelete!(selected)}>
            <Trash2 size={15} />
          </IconButton>
        )}
        <IconButton label="Refresh" onClick={p.onRefresh}>
          <RefreshCw size={15} />
        </IconButton>
        <Button size="sm" variant="primary" disabled={!n} onClick={() => p.onTransfer(selected)}>
          {p.transferIcon} {p.transferLabel}
          {n > 1 ? ` ${n} items` : ""}
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
      <div
        role="listbox"
        aria-multiselectable
        aria-label={p.title}
        tabIndex={0}
        onKeyDown={onKey}
        className="min-h-0 flex-1 select-none overflow-y-auto py-1 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent/30"
      >
        {p.loading ? (
          <div className="grid h-full place-items-center px-4 text-center text-caption text-muted">{p.loadingText ?? "Connecting…"}</div>
        ) : p.entries.length === 0 ? (
          <div className="grid h-full place-items-center text-caption text-muted">Empty folder</div>
        ) : (
          p.entries.map((e) => (
            <div
              key={e.name}
              role="option"
              aria-selected={sel.names.includes(e.name)}
              ref={sel.cursor === e.name ? (el) => el?.scrollIntoView({ block: "nearest" }) : undefined}
              draggable
              onDragStart={(ev) => {
                // Dragging a selected row takes the whole selection, any other row only itself.
                const drag = sel.names.includes(e.name) ? sel.names : [e.name];
                if (drag.length === 1) setSel(clickSelect(names, sel, e.name));
                ev.dataTransfer.setData(`x-tether/${p.onDropFrom === "local" ? "remote" : "local"}`, JSON.stringify(drag));
              }}
              onClick={(ev) => setSel(clickSelect(names, sel, e.name, { toggle: ev.ctrlKey || ev.metaKey, range: ev.shiftKey }))}
              onDoubleClick={() => (isDir(e) ? p.onOpen(e) : p.onTransfer([e]))}
              className={`mx-1 flex cursor-default items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-caption ${sel.names.includes(e.name) ? "bg-accent/15" : "hover:bg-parchment"}`}
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

function Jobs({ jobs, onCancel, onClear }: { jobs: Job[]; onCancel: (j: Job) => void; onClear: () => void }) {
  if (!jobs.length) return <div className="h-4" />;
  const finished = jobs.filter((j) => j.state === "done" || j.state === "error").length;
  const total = jobs.reduce((a, j) => a + j.total, 0);
  const done = jobs.reduce((a, j) => a + (j.state === "done" ? j.total : Math.min(j.done, j.total)), 0);
  return (
    <div className="m-4 mt-3 max-h-40 overflow-y-auto rounded-2xl border border-hairline bg-tile">
      <div className="flex items-center justify-between px-3 py-2">
        <span className="text-caption">
          <span className="font-semibold">Transfers</span>
          <span className="ml-2 text-muted" data-testid="ft-overall">
            {finished} of {jobs.length} finished{total && finished < jobs.length ? ` · ${Math.floor((done / total) * 100)}%` : ""}
          </span>
        </span>
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
              {j.state === "queued" ? "Queued" : j.state === "running" ? `${fmtSize(j.speed)}/s` : j.state === "done" ? <CheckCircle2 size={15} className="ml-auto text-ok" /> : <span className="text-danger">{j.err ?? "failed"}</span>}
            </span>
            {(j.state === "running" || j.state === "queued") && (
              <button aria-label={`Cancel ${j.name}`} className="text-muted hover:text-danger" onClick={() => onCancel(j)}>
                <XCircle size={15} />
              </button>
            )}
          </div>
        );
      })}
    </div>
  );
}
