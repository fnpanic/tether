import { createContext, useCallback, useContext, useEffect, useState } from "react";
import { Activity, FileCode2, KeyRound, Lock, Monitor, Package, ShieldCheck, Stethoscope } from "lucide-react";
import { api, emptyBundle, invoke, type AppState, type Bundle } from "./lib/ipc";
import { useToast } from "./components/ui";
import { Gate } from "./pages/Gate";
import { Home } from "./pages/Home";
import { Policy } from "./pages/Policy";
import { Devices } from "./pages/Devices";
import { Releases } from "./pages/Releases";
import { DownloadPage } from "./pages/DownloadPage";
import { Diagnostics } from "./pages/Diagnostics";
import { Keys } from "./pages/Keys";

interface Ctx {
  state: AppState;
  reload: () => Promise<void>;
  draft: Bundle;
  setDraft: (b: Bundle) => void;
  /** Bundle content differs from the last published one (assignments edited etc.). */
  dirty: boolean;
  markPublished: (b: Bundle) => void;
  toast: (m: string) => void;
  go: (p: PageId) => void;
}
const AppCtx = createContext<Ctx>(null!);
export const useApp = () => useContext(AppCtx);

const PAGES = [
  { id: "home", label: "Status", icon: Activity, el: Home },
  { id: "policy", label: "Policy", icon: ShieldCheck, el: Policy },
  { id: "devices", label: "Devices", icon: Monitor, el: Devices },
  { id: "releases", label: "Releases", icon: Package, el: Releases },
  { id: "page", label: "Download page", icon: FileCode2, el: DownloadPage },
  { id: "diag", label: "Diagnostics", icon: Stethoscope, el: Diagnostics },
  { id: "keys", label: "Keys and server", icon: KeyRound, el: Keys },
] as const;
export type PageId = (typeof PAGES)[number]["id"];

const content = (b: Bundle) => JSON.stringify({ ...b, serial: 0, issued_at: 0 });

export function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [page, setPage] = useState<PageId>("home");
  const [draft, setDraftState] = useState<Bundle>(emptyBundle());
  const [published, setPublished] = useState("");
  const { show, node } = useToast();

  const reload = useCallback(async () => setState(await api.state()), []);
  useEffect(() => void reload(), [reload]);

  useEffect(() => {
    if (!state?.unlocked) return;
    invoke<Bundle>("policy_draft")
      .then((b) => {
        setDraftState(b);
        setPublished(content(b));
      })
      .catch((e) => show(e.message));
  }, [state?.unlocked]);

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
    dirty: content(draft) !== published,
    markPublished: (b) => {
      setDraftState(b);
      setPublished(content(b));
    },
    toast: show,
    go: setPage,
  };

  return (
    <AppCtx.Provider value={ctx}>
      <div className="flex h-full">
        <nav className="flex w-56 shrink-0 flex-col gap-0.5 border-r border-hairline bg-parchment p-3">
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
              className={`flex h-9 items-center gap-2.5 rounded-md px-2.5 text-left text-caption ${page === id ? "bg-accent/12 text-ink" : "text-ink-80 hover:bg-tile-2"}`}
            >
              <Icon size={16} strokeWidth={1.75} className={page === id ? "text-accent" : "text-muted"} />
              {label}
              {id === "policy" && ctx.dirty && <span className="ml-auto h-1.5 w-1.5 rounded-full bg-accent" title="Unpublished changes" />}
            </button>
          ))}
          <div className="mt-auto px-2.5 pb-1">
            <p className="truncate text-footnote text-muted" title={state.settings.server}>
              {state.settings.server.replace(/^https:\/\//, "")}
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
        <main className="min-w-0 flex-1 overflow-auto">
          <div className="mx-auto max-w-[1180px] px-8 py-8">
            <Page />
          </div>
        </main>
      </div>
      {node}
    </AppCtx.Provider>
  );
}
