import { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { Activity, FileCode2, History, KeyRound, ListChecks, Lock, LogOut, Monitor, Package, Palette, ShieldCheck, Stethoscope, Unlock, Users } from "lucide-react";
import { api, emptyBundle, invoke, type AppState, type Bundle, type Change, type Live } from "./lib/ipc";
import { incomplete } from "./lib/live";
import { toRenew, type GrantsInfo } from "./lib/grants";
import type { TeamDoc } from "./lib/teamdoc";
import { access, can, pageAllowed, ROLE_TEXT, type Access, type Action } from "./lib/roles";
import { SELECTED, useToast } from "./components/ui";
import { PolicyBar, PolicyPublish } from "./components/policy-publish";
import { Gate } from "./pages/Gate";
import { Home } from "./pages/Home";
import { Policy } from "./pages/Policy";
import { Devices } from "./pages/Devices";
import { Sessions } from "./pages/Sessions";
import { Team } from "./pages/Team";
import { Releases } from "./pages/Releases";
import { DownloadPage } from "./pages/DownloadPage";
import { CustomApps } from "./pages/CustomApps";
import { Diagnostics } from "./pages/Diagnostics";
import { TeamActivity } from "./pages/TeamActivity";
import { Keys } from "./pages/Keys";

interface Ctx {
  state: AppState;
  reload: () => Promise<void>;
  draft: Bundle;
  setDraft: (b: Bundle) => void;
  /** What the relays serve now; null while loading or when unreachable. */
  live: Live | null;
  /** Why the last live check failed (the relays' answers stay in `live`). */
  liveError: string | null;
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
  /** Unpublished changes to the team document: kept while another page is open (Team page). */
  teamDraft: TeamDoc | null;
  setTeamDraft: React.Dispatch<React.SetStateAction<TeamDoc | null>>;
  /** The keys on this PC and the sign-in's Tether Manage roles. */
  access: Access;
  can: (x: Action) => boolean;
  /** The proposal a key holder loaded into the policy draft (removed from the relays once published). */
  reviewing: Proposal | null;
  setReviewing: (p: Proposal | null) => void;
}

/** A proposal as proposals_list merges it over the relays. */
export interface Proposal {
  kind: "policy" | "team" | "release";
  author: { key_fingerprint: string; name: string; oid?: string; upn?: string };
  created: number;
  note: string;
  serial?: number;
  doc: any;
  ids: Record<string, string>;
  relays: string[];
  /** Relays that serve something else now than what it was based on. */
  stale: string[];
}
const AppCtx = createContext<Ctx>(null!);
export const useApp = () => useContext(AppCtx);

const PAGES = [
  { id: "home", label: "Status and relays", icon: Activity, el: Home },
  { id: "policy", label: "Policy", icon: ShieldCheck, el: Policy },
  { id: "devices", label: "Devices", icon: Monitor, el: Devices },
  { id: "sessions", label: "Sessions", icon: History, el: Sessions },
  { id: "activity", label: "Team activity", icon: ListChecks, el: TeamActivity },
  { id: "team", label: "Team", icon: Users, el: Team },
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
  const [liveError, setLiveError] = useState<string | null>(null);
  const [changes, setChanges] = useState<Change[]>([]);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [publishing, setPublishing] = useState(false);
  const [teamDraft, setTeamDraft] = useState<TeamDoc | null>(null);
  const [reviewing, setReviewing] = useState<Proposal | null>(null);
  // A signed-in key holder unlocks the keys from here.
  const [unlocking, setUnlocking] = useState(false);
  const { show, node } = useToast();

  const reload = useCallback(async () => setState(await api.state()), []);
  const acc = access(!!state?.unlocked, state?.signin);
  const usable = acc.vault || (acc.session && acc.roles.length > 0);
  useEffect(() => void reload(), [reload]);

  // Only the newest check's answer counts: an older, slower one must not overwrite it.
  const liveSeq = useRef(0);
  // Checks answered so far: each one arms the next retry, also when it failed the same way.
  const [checks, setChecks] = useState(0);
  const reloadLive = useCallback(() => {
    const n = ++liveSeq.current;
    return api.live().then(
      (l) => void (n === liveSeq.current && (setLive(l), setLiveError(null), setChecks((c) => c + 1))),
      (e) => void (n === liveSeq.current && (setLiveError(e.message), setChecks((c) => c + 1))),
    );
  }, []);

  useEffect(() => {
    if (!usable) return;
    invoke<Bundle>("policy_draft").then(
      (b) => {
        setDraftState(b);
        // Grants are renewed while the keys are unlocked (docs/management.md).
        if (can(acc, "sign")) invoke<GrantsInfo>("grants_info", { bundle: b }).then((g) => {
          const due = toRenew(g);
          if (due.length) show(`Health monitoring: renew the enrolment grant of ${due.map(([k]) => k.replace(/^profile:(.*)$/, "the $1 profile").replace(/^group:/, "group ")).join(", ")} on the Policy page, then publish.`);
        }, () => {});
      },
      (e) => show(e.message),
    );
    reloadLive();
  }, [usable, state?.unlocked, state?.signin.signed_in, state?.settings.server]);

  // A check that failed or that a relay did not answer (network not up yet, a relay restarting)
  // is asked again every 30 s until every relay answers; before, it stayed as it was until a
  // manual "Check again".
  const retry = usable && incomplete(live, liveError);
  useEffect(() => {
    if (!retry) return;
    const t = setTimeout(reloadLive, 30_000);
    return () => clearTimeout(t);
  }, [retry, checks]);

  useEffect(() => {
    if (!usable) return;
    invoke<Change[]>("policy_diff", { from: live?.bundle ?? null, to: draft }).then(setChanges, () => setChanges([]));
    invoke("policy_check", { bundle: draft }).then(() => setInvalid(null), (e) => setInvalid(e.message));
  }, [draft, live, usable]);

  const setDraft = (b: Bundle) => {
    setDraftState(b);
    invoke("policy_save_draft", { bundle: b }).catch((e) => show(`Could not save the draft: ${e.message}`));
  };

  if (!state) return null;
  if (!usable || unlocking)
    return (
      <Gate
        state={state}
        onDone={() => {
          setUnlocking(false);
          reload();
        }}
        onCancel={unlocking ? () => setUnlocking(false) : undefined}
      />
    );

  const pages = PAGES.filter((p) => pageAllowed(p.id, acc));
  const Page = (pages.find((p) => p.id === page) ?? pages[0]).el;
  const signOut = async () => {
    await invoke("signin_out");
    setReviewing(null);
    await reload();
  };
  const ctx: Ctx = {
    state,
    reload,
    draft,
    setDraft,
    live,
    liveError,
    reloadLive,
    changes,
    dirty: changes.length > 0,
    invalid,
    // Without signing rights the policy page proposes (policy editors) or only shows.
    publish: () => (can(acc, "sign") ? setPublishing(true) : setPage("policy")),
    toast: show,
    go: setPage,
    teamDraft,
    setTeamDraft,
    access: acc,
    can: (x) => can(acc, x),
    reviewing,
    setReviewing,
  };

  return (
    <AppCtx.Provider value={ctx}>
      <div className="flex h-full">
        <nav className="flex w-56 shrink-0 flex-col gap-0.5 border-r border-hairline bg-parchment p-3" aria-label="Sections">
          <div className="mb-4 flex items-center gap-2 px-2 pt-2">
            <svg width={22} height={22} viewBox="0 0 1024 1024" aria-hidden className="text-on-accent">
              <rect x="64" y="64" width="896" height="896" rx="216" fill="var(--accent)" />
              <rect x="236" y="300" width="352" height="248" rx="44" fill="none" stroke="currentColor" strokeWidth="56" />
              <rect x="436" y="476" width="352" height="248" rx="44" fill="currentColor" fillOpacity="0.25" stroke="currentColor" strokeWidth="56" />
            </svg>
            <span className="text-callout font-semibold">Tether Manage</span>
          </div>
          {pages.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setPage(id)}
              aria-current={page === id ? "page" : undefined}
              className={`flex h-9 items-center gap-2.5 rounded-md px-2.5 text-left text-caption ${page === id ? SELECTED : "text-ink-80 hover:bg-tile-2"}`}
            >
              <Icon size={16} strokeWidth={1.75} className={page === id ? "text-on-accent" : "text-muted"} />
              {label}
              {id === "policy" && ctx.dirty && (
                <span className={`ml-auto rounded-full px-1.5 text-footnote ${page === id ? "bg-on-accent/25 text-on-accent" : "bg-accent text-on-accent"}`} title="Unpublished changes">
                  {changes.length}
                </span>
              )}
            </button>
          ))}
          <div className="mt-auto px-2.5 pb-1">
            <p className="truncate text-footnote text-muted" title={state.servers.join("\n")}>
              {state.servers.length} relay{state.servers.length === 1 ? "" : "s"}
            </p>
            {acc.session && (
              <div className="mt-2 text-footnote">
                <p className="truncate font-semibold" title={state.signin.upn}>
                  {state.signin.name}
                </p>
                <p className="text-muted">{acc.roles.length ? acc.roles.map((r) => ROLE_TEXT[r]).join(", ") : "No Tether Manage role"}</p>
                {state.signin.alarm && (
                  <p role="alert" className="mt-1 text-danger">
                    {state.signin.alarm}
                  </p>
                )}
                <button onClick={signOut} className="mt-1 flex items-center gap-1.5 text-accent">
                  <LogOut size={12} /> Sign out
                </button>
              </div>
            )}
            {acc.vault ? (
              <button
                onClick={async () => {
                  await invoke("keys_lock");
                  await reload();
                }}
                className="mt-2 flex items-center gap-1.5 text-footnote text-accent"
              >
                <Lock size={12} /> Lock
              </button>
            ) : (
              acc.roles.includes("key-holder") &&
              state.exists && (
                <button onClick={() => setUnlocking(true)} className="mt-2 flex items-center gap-1.5 text-footnote text-accent">
                  <Unlock size={12} /> Unlock the keys
                </button>
              )
            )}
            {acc.vault && acc.session && !can(acc, "sign") && <p className="mt-1 text-footnote text-warn">Signed in without the KeyHolder role: the keys do not sign.</p>}
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
