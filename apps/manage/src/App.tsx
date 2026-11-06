import { createContext, useCallback, useContext, useEffect, useState } from "react";
import { Activity, FileCode2, History, KeyRound, Lock, Monitor, Package, Palette, ShieldCheck, Stethoscope } from "lucide-react";
import { api, emptyBundle, invoke, type AppState, type Bundle, type Change, type Live } from "./lib/ipc";
import { SELECTED, useToast } from "./components/ui";
import { PolicyBar, PolicyPublish } from "./components/policy-publish";
import { Gate } from "./pages/Gate";
import { Home } from "./pages/Home";
import { Policy } from "./pages/Policy";
import { Devices } from "./pages/Devices";
import { Sessions } from "./pages/Sessions";
import { Releases } from "./pages/Releases";
import { DownloadPage } from "./pages/DownloadPage";
import { CustomApps } from "./pages/CustomApps";
import { Diagnostics } from "./pages/Diagnostics";
import { Keys } from "./pages/Keys";

interface Ctx {
  state: AppState;
  reload: () => Promise<void>;
  draft: Bundle;
  setDraft: (b: Bundle) => void;
  /** What the relays serve now; null while loading or when unreachable. */
  live: Live | null;
  reloadLive: () => Promise<void>;
  /** Draft compared with the live policy. */
  changes: Change[];
  dirty: boolean;
  /** Why the draft cannot be published, if it cannot. */
  invalid: string | null;
  /** Opens the publish review sheet. */
  publish: () => void;
  toast: (m: string) => void;
  go: (p: PageId) => void;
}
const AppCtx = createContext<Ctx>(null!);
export const useApp = () => useContext(AppCtx);

const PAGES = [
  { id: "home", label: "Status and relays", icon: Activity, el: Home },
  { id: "policy", label: "Policy", icon: ShieldCheck, el: Policy },
  { id: "devices", label: "Devices", icon: Monitor, el: Devices },
  { id: "sessions", label: "Sessions", icon: History, el: Sessions },
  { id: "releases", label: "Releases", icon: Package, el: Releases },
  { id: "page", label: "Download page", icon: FileCode2, el: DownloadPage },
  { id: "custom", label: "Custom apps", icon: Palette, el: CustomApps },
  { id: "diag", label: "Diagnostics", icon: Stethoscope, el: Diagnostics },
  { id: "keys", label: "Keys", icon: KeyRound, el: Keys },
] as const;
export type PageId = (typeof PAGES)[number]["id"];

export function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [page, setPage] = useState<PageId>("home");
  const [draft, setDraftState] = useState<Bundle>(emptyBundle());
  const [live, setLive] = useState<Live | null>(null);
  const [changes, setChanges] = useState<Change[]>([]);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [publishing, setPublishing] = useState(false);
  const { show, node } = useToast();

  const reload = useCallback(async () => setState(await api.state()), []);
  useEffect(() => void reload(), [reload]);

  const reloadLive = useCallback(() => api.live().then(setLive, (e) => show(`Could not read the live policy: ${e.message}`)), []);

  useEffect(() => {
    if (!state?.unlocked) return;
    invoke<Bundle>("policy_draft").then(setDraftState, (e) => show(e.message));
    reloadLive();
  }, [state?.unlocked, state?.settings.server]);

  useEffect(() => {
    if (!state?.unlocked) return;
    invoke<Change[]>("policy_diff", { from: live?.bundle ?? null, to: draft }).then(setChanges, () => setChanges([]));
    invoke("policy_check", { bundle: draft }).then(() => setInvalid(null), (e) => setInvalid(e.message));
  }, [draft, live, state?.unlocked]);

  const setDraft = (b: Bundle) => {
    setDraftState(b);
    invoke("policy_save_draft", { bundle: b }).catch((e) => show(`Could not save the draft: ${e.message}`));
  };

  if (!state) return null;
  if (!state.unlocked) return <Gate state={state} onDone={reload} />;

  const Page = PAGES.find((p) => p.id === page)!.el;
  const ctx: Ctx = {
    state,
    reload,
    draft,
    setDraft,
    live,
    reloadLive,
    changes,
    dirty: changes.length > 0,
    invalid,
    publish: () => setPublishing(true),
    toast: show,
    go: setPage,
  };

  return (
    <AppCtx.Provider value={ctx}>
      <div className="flex h-full">
        <nav className="flex w-56 shrink-0 flex-col gap-0.5 border-r border-hairline bg-parchment p-3" aria-label="Sections">
          <div className="mb-4 flex items-center gap-2 px-2 pt-2">
            <svg width={22} height={22} viewBox="0 0 1024 1024" aria-hidden>
              <rect x="64" y="64" width="896" height="896" rx="216" fill="var(--accent)" />
              <rect x="236" y="300" width="352" height="248" rx="44" fill="none" stroke="#fff" strokeWidth="56" />
              <rect x="436" y="476" width="352" height="248" rx="44" fill="#fff" fillOpacity="0.25" stroke="#fff" strokeWidth="56" />
            </svg>
            <span className="text-callout font-semibold">Tether Manage</span>
          </div>
          {PAGES.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setPage(id)}
              aria-current={page === id ? "page" : undefined}
              className={`flex h-9 items-center gap-2.5 rounded-md px-2.5 text-left text-caption ${page === id ? SELECTED : "text-ink-80 hover:bg-tile-2"}`}
            >
              <Icon size={16} strokeWidth={1.75} className={page === id ? "text-white" : "text-muted"} />
              {label}
              {id === "policy" && ctx.dirty && (
                <span className={`ml-auto rounded-full px-1.5 text-footnote ${page === id ? "bg-white/25 text-white" : "bg-accent text-white"}`} title="Unpublished changes">
                  {changes.length}
                </span>
              )}
            </button>
          ))}
          <div className="mt-auto px-2.5 pb-1">
            <p className="truncate text-footnote text-muted" title={state.servers.join("\n")}>
              {state.servers.length} relay{state.servers.length === 1 ? "" : "s"}
            </p>
            <button
              onClick={async () => {
                await invoke("keys_lock");
                await reload();
              }}
              className="mt-2 flex items-center gap-1.5 text-footnote text-accent"
            >
              <Lock size={12} /> Lock
            </button>
          </div>
        </nav>
        <main className="flex min-w-0 flex-1 flex-col">
          <PolicyBar />
          <div className="min-h-0 flex-1 overflow-auto">
            <div className="mx-auto max-w-[1180px] px-8 py-8">
              <Page />
            </div>
          </div>
        </main>
      </div>
      {publishing && <PolicyPublish onClose={() => setPublishing(false)} />}
      {node}
    </AppCtx.Provider>
  );
}
