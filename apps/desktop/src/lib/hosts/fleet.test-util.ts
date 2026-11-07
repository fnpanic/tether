// Deterministic hosts for tests (same shape as the mock backend's fleet).
import { buildHosts, emptyStore, type Host, type OrgDevice, type Peer, type Store } from "./model";
import { applyRules } from "./rules";

export function fleet(n: number, now = 1_800_000_000): { store: Store; peers: Peer[]; org: OrgDevice[]; hosts: Host[]; online: Map<string, boolean> } {
  let seed = 11;
  const rnd = () => (seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff) / 0x7fffffff;
  const pick = <T,>(a: T[]) => a[Math.floor(rnd() * a.length)];
  const sites = ["BER", "MUC", "HAM", "FRA", "VIE", "ZRH", "LON", "NYC"];
  const kinds: [string, string][] = [["WS", "Windows"], ["LT", "Windows"], ["SRV", "Windows"], ["MAC", "Mac OS"], ["LNX", "Linux"]];
  const store = emptyStore();
  const peers: Peer[] = [];
  const org: OrgDevice[] = [];
  const online = new Map<string, boolean>();
  for (let i = 0; i < n; i++) {
    const site = pick(sites);
    const [kind, platform] = pick(kinds);
    const id = String(100000000 + i * 7919);
    peers.push({ id, hostname: `${site}-${kind}-${String(i).padStart(4, "0")}`, username: pick(["anna", "ben", "c.mueller", "front"]), platform, last: now - Math.floor(rnd() * 90 * 86400) });
    if (i % 2) org.push({ uuid: `u${i}`, id, profile: "customer", version: pick(["0.5.19", "0.5.20", "0.5.21"]), os: platform === "Windows" ? "windows 10.0.22631" : platform === "Mac OS" ? "macos 15.1" : "ubuntu 24.04", last_seen: now - Math.floor(rnd() * 20 * 86400) });
    if (i % 3 === 0) store.hosts[id] = { tags: [`site:${site.toLowerCase()}`, pick(["customer:acme", "customer:globex", "customer:initech"]), ...(kind === "SRV" ? ["env:prod"] : [])], ...(i % 10 === 0 ? { notes: `rack ${i % 40}, call ${pick(["Maria", "Jonas"])} first` } : {}), ...(i % 50 === 0 ? { pinned: true } : {}) };
    online.set(id, rnd() < 0.4);
  }
  store.rules = [{ id: "r1", tag: "platform:mac", when: "os:mac" }, { id: "r2", tag: "site:berlin", when: "host:BER-" }];
  const hosts = buildHosts(store, peers, org, online, now);
  applyRules(hosts, store.rules, now);
  return { store, peers, org, hosts, online };
}
