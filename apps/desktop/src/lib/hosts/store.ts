// Advanced mode state: the persisted host store plus runtime sources (peers, org devices, online).
// Saves are debounced and asynchronous (the backend writes on a worker thread), so typing a note
// or tagging 5,000 hosts never waits for the disk.
import { create } from "zustand";
import { invoke } from "../ipc";
import { cleanTags, emptyStore, MAX_NAME, MAX_NOTES, shared, applyShared, STORE_VERSION, type HostEntry, type OrgDevice, type Peer, type SharedDoc, type Store } from "./model";
import { merge3, readShared } from "./sync";

export type OnlineState = "idle" | "loading" | "ok" | "unsupported" | "error";

export interface TeamInfo {
  identity?: { key: string; fingerprint: string };
  me?: { name: string; write: boolean; audit: boolean } | null;
  error?: string;
  syncing?: boolean;
}

interface S {
  loaded: boolean;
  store: Store;
  peers: Peer[];
  org: OrgDevice[];
  online: Map<string, boolean> | null;
  onlineState: OnlineState;
  team: TeamInfo;
  saveError?: string;
}

export const useHosts = create<S>(() => ({
  loaded: false,
  store: emptyStore(),
  peers: [],
  org: [],
  online: null,
  onlineState: "idle",
  team: {},
}));

const get = () => useHosts.getState();
const set = (p: Partial<S>) => useHosts.setState(p);

function readStore(raw: string): Store {
  if (!raw) return emptyStore();
  try {
    const d = JSON.parse(raw) as Partial<Store>;
    if (!d || typeof d !== "object") return emptyStore();
    return { ...emptyStore(), ...d, v: STORE_VERSION, hosts: d.hosts ?? {}, tags: d.tags ?? {}, rules: d.rules ?? [], searches: d.searches ?? [] };
  } catch {
    return emptyStore();
  }
}

let saveTimer = 0;
let teamTimer = 0;
let saving: Promise<void> = Promise.resolve();

export function flush(): Promise<void> {
  if (saveTimer) {
    clearTimeout(saveTimer);
    saveTimer = 0;
    const doc = JSON.stringify(get().store);
    saving = saving.then(() =>
      invoke("hosts_store_save", { doc }).then(
        () => set({ saveError: undefined }),
        (e) => set({ saveError: String(e) }),
      ),
    );
  }
  return saving;
}

/** Replace the store; `sharedChange` schedules a team sync. */
export function commit(next: Store, sharedChange = true) {
  set({ store: next });
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(flush, 400);
  if (sharedChange && next.team?.enabled) {
    clearTimeout(teamTimer);
    teamTimer = window.setTimeout(() => void teamSync(), 2500);
  }
}

function edit(ids: string[], f: (e: HostEntry, id: string) => HostEntry, sharedChange = true) {
  const s = get().store;
  const hosts = { ...s.hosts };
  for (const id of ids) {
    const e = f({ ...(hosts[id] ?? {}) }, id);
    // Drop keys that became empty so the file stays small.
    for (const k of Object.keys(e) as (keyof HostEntry)[]) {
      const v = e[k];
      if (v === undefined || v === "" || v === false || (Array.isArray(v) && !v.length)) delete e[k];
    }
    if (Object.keys(e).length) hosts[id] = e;
    else delete hosts[id];
  }
  commit({ ...s, hosts }, sharedChange);
}

export const actions = {
  async load() {
    const [raw, peers] = await Promise.all([invoke<string>("hosts_store_load").catch(() => ""), invoke<Peer[]>("hosts_peers").catch(() => [])]);
    set({ store: readStore(raw ?? ""), peers: peers ?? [], loaded: true });
    if (get().store.team?.enabled) {
      void actions.teamRefresh();
    }
  },
  async reloadPeers() {
    set({ peers: (await invoke<Peer[]>("hosts_peers").catch(() => null)) ?? get().peers });
  },
  async refreshOnline() {
    const ids = [...new Set([...get().peers.map((p) => p.id), ...Object.keys(get().store.hosts), ...get().org.map((d) => d.id)])];
    if (!ids.length) return;
    set({ onlineState: get().online ? get().onlineState : "loading" });
    try {
      const on = await invoke<string[]>("hosts_online", { ids });
      const m = new Map<string, boolean>(ids.map((id) => [id, false]));
      for (const id of on ?? []) m.set(id, true);
      set({ online: m, onlineState: "ok" });
    } catch (e) {
      set({ onlineState: String(e).startsWith("unsupported") ? "unsupported" : "error" });
    }
  },
  addTags(ids: string[], tags: string[]) {
    const t = cleanTags(tags);
    if (t.length) edit(ids, (e) => ({ ...e, tags: cleanTags([...(e.tags ?? []), ...t]) }));
  },
  removeTags(ids: string[], tags: string[]) {
    edit(ids, (e) => ({ ...e, tags: (e.tags ?? []).filter((x) => !tags.includes(x)) }));
  },
  setName(id: string, name: string) {
    edit([id], (e) => ({ ...e, name: name.trim().slice(0, MAX_NAME) }));
  },
  setNotes(id: string, notes: string) {
    edit([id], (e) => ({ ...e, notes: notes.slice(0, MAX_NOTES) }), false);
  },
  setPinned(ids: string[], pinned: boolean) {
    edit(ids, (e) => ({ ...e, pinned }), false);
  },
  addHost(id: string, name: string, tags: string[]) {
    edit([id], (e) => ({ ...e, added: e.added ?? Math.floor(Date.now() / 1000), name: name.trim().slice(0, MAX_NAME) || e.name, tags: cleanTags([...(e.tags ?? []), ...tags]) }));
  },
  /** Forget a host: its store entry and the core's recent entry. Team-shared tags go on the next sync. */
  async remove(ids: string[]) {
    edit(ids, () => ({}));
    const recent = new Set(get().peers.map((p) => p.id));
    for (const id of ids) if (recent.has(id)) await invoke("remove_peer", { id }).catch(() => {});
    await actions.reloadPeers();
  },
  logSession(id: string, conn: string) {
    edit([id], (e) => ({ ...e, history: [{ t: Math.floor(Date.now() / 1000), conn }, ...(e.history ?? [])].slice(0, 50) }), false);
  },
  setStore(f: (s: Store) => Store, sharedChange = true) {
    commit(f(get().store), sharedChange);
  },
  setTeam(p: Partial<NonNullable<Store["team"]>>) {
    const s = get().store;
    commit({ ...s, team: { enabled: false, ...s.team, ...p } }, false);
  },
  /** Identity, membership and org devices; then a tag sync. */
  async teamRefresh() {
    const team: TeamInfo = { ...get().team, error: undefined };
    try {
      team.identity ??= await invoke<{ key: string; fingerprint: string }>("team_identity");
      team.me = await invoke<TeamInfo["me"]>("team_me");
    } catch (e) {
      team.me = null;
      team.error = String(e).replace(/^(not-member|unsupported|offline): /, "");
      set({ team, org: [] });
      return;
    }
    set({ team });
    if (get().store.team?.showOrg !== false) {
      const org = await invoke<OrgDevice[]>("team_devices").catch(() => null);
      if (org) set({ org });
    }
    await teamSync();
  },
};

const sameDoc = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** Pull, merge, push (on a conflict: merge again with what won, up to 3 times). */
export async function teamSync(): Promise<void> {
  const st = get();
  if (!st.store.team?.enabled || st.team.syncing || !st.team.me) return;
  set({ team: { ...st.team, syncing: true } });
  try {
    let cur = (await invoke<{ rev: number; doc: unknown }>("team_tags_get")) ?? { rev: 0, doc: null };
    for (let attempt = 0; attempt < 3; attempt++) {
      const remote = readShared(cur.doc);
      const store = get().store;
      const merged: SharedDoc = merge3(store.team?.base ?? null, shared(store), remote);
      const canWrite = !!get().team.me?.write;
      if (!canWrite || sameDoc(merged, remote)) {
        // Read-only members take the team's doc as it is.
        const doc = canWrite ? merged : (remote ?? merged);
        commit({ ...applyShared(store, doc), team: { ...store.team!, base: doc, rev: cur.rev, synced: Date.now() } }, false);
        break;
      }
      const r = await invoke<{ ok: boolean; rev?: number; conflict?: { rev: number; doc: unknown } }>("team_tags_put", { rev: cur.rev, doc: JSON.stringify(merged) });
      if (r.ok) {
        const now = get().store;
        commit({ ...applyShared(now, merge3(store.team?.base ?? null, shared(now), merged)), team: { ...now.team!, base: merged, rev: r.rev ?? cur.rev + 1, synced: Date.now() } }, false);
        break;
      }
      cur = r.conflict ?? cur;
    }
    set({ team: { ...get().team, syncing: false, error: undefined } });
  } catch (e) {
    set({ team: { ...get().team, syncing: false, error: String(e).replace(/^(not-member|unsupported|offline): /, "") } });
  }
}
