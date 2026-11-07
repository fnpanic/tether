// Mock mode only: a deterministic fleet for Advanced mode (`?hosts=5000` in the page URL; default 1200).
export type Fleet = ReturnType<typeof fleet>;

export function fleet() {
  const n = Math.max(0, Math.min(20000, Number(new URLSearchParams(location.search).get("hosts") ?? 1200) || 0));
  let seed = 7;
  const rnd = () => ((seed = (Math.imul(seed, 1103515245) + 12345) & 0x7fffffff) / 0x7fffffff);
  const pick = <T,>(a: T[]) => a[Math.floor(rnd() * a.length)];
  const sites = ["BER", "MUC", "HAM", "FRA", "VIE", "ZRH", "LON", "NYC"];
  const kinds = [["WS", "Windows"], ["LT", "Windows"], ["SRV", "Windows"], ["MAC", "Mac OS"], ["LNX", "Linux"], ["KIOSK", "Windows"]] as const;
  const users = ["anna", "ben", "c.mueller", "d.schmidt", "eva", "f.weber", "gina", "h.koch", "front", "ci", "lab"];
  const now = Math.floor(Date.now() / 1000);
  const peers: { id: string; hostname: string; username: string; platform: string; last: number }[] = [];
  const org: { uuid: string; id: string; profile: string; version: string; os: string; last_seen: number }[] = [];
  const online: string[] = [];
  for (let i = 0; i < n; i++) {
    const site = pick(sites);
    const [kind, platform] = pick(kinds as unknown as (readonly [string, string])[]);
    const id = String(100000000 + Math.floor(rnd() * 899999999));
    const hostname = `${site}-${kind}-${String(i % 1000).padStart(3, "0")}`;
    const os = platform === "Windows" ? pick(["windows 10.0.19045", "windows 10.0.22631", "windows 10.0.26100"]) : platform === "Mac OS" ? pick(["macos 14.6", "macos 15.1"]) : pick(["ubuntu 24.04", "debian 12"]);
    if (i % 3 !== 2) peers.push({ id, hostname, username: pick(users), platform, last: now - Math.floor(rnd() * 120 * 86400) });
    if (i % 4 !== 0) org.push({ uuid: `u-${i}`, id, profile: i % 9 === 0 ? "technician" : "customer", version: pick(["0.5.19", "0.5.20", "0.5.21", "0.5.21"]), os, last_seen: now - Math.floor(rnd() * 30 * 86400) });
    if (rnd() < 0.42) online.push(id);
  }
  // A lived-in store for the demo and screenshots: tags on most hosts, rules, saved searches.
  const cust = ["acme", "globex", "initech", "umbrella", "stark"];
  const hosts: Record<string, { tags?: string[]; name?: string; notes?: string; pinned?: boolean }> = {};
  [...peers, ...org].forEach((h, i) => {
    if (i % 5 === 4) return;
    const name = "hostname" in h ? h.hostname : "";
    const site = { BER: "berlin", MUC: "munich", HAM: "hamburg", FRA: "frankfurt", VIE: "vienna", ZRH: "zurich", LON: "london", NYC: "new-york" }[name.slice(0, 3)] ?? "remote";
    hosts[h.id] = {
      tags: [`customer:${cust[i % cust.length]}`, ...(i % 2 ? [`site:${site}`] : []), ...(name.includes("SRV") ? ["env:prod"] : i % 11 === 0 ? ["env:test"] : []), ...(i % 17 === 0 ? ["vip"] : [])],
      ...(i % 13 === 0 ? { notes: "Call the front desk before restarting." } : {}),
      ...(i % 40 === 7 ? { pinned: true } : {}),
    };
  });
  const store = {
    v: 1,
    hosts,
    tags: { customer: { color: "blue" }, site: { color: "teal" }, env: { color: "orange" }, vip: { color: "pink" } },
    rules: [
      { id: "r1", tag: "platform:mac", when: "os:mac" },
      { id: "r2", tag: "role:server", when: "host:-SRV- | host:-LNX-" },
      { id: "r3", tag: "update:needed", when: "ver:<0.5.21 is:org" },
    ],
    searches: [
      { id: "s1", name: "Acme servers", query: "#customer:acme #role:server" },
      { id: "s2", name: "Berlin, online", query: "#site:berlin is:online" },
      { id: "s3", name: "Needs update", query: "#update:needed" },
    ],
    team: { enabled: true, showOrg: true },
  };
  return { peers, org, online: new Set(online), store: JSON.stringify(store) };
}
