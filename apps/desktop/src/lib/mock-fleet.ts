// Mock mode only: a deterministic fleet for Advanced mode (`?hosts=5000` in the page URL; default 1200).
import type { ReportCheck, StatusChange, StatusFault, StatusHostReply, StatusProduct, StatusSummary, Text2 } from "./hosts/model";

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
  const names = new Map<string, string>();
  for (let i = 0; i < n; i++) {
    const site = pick(sites);
    const [kind, platform] = pick(kinds as unknown as (readonly [string, string])[]);
    const id = String(100000000 + Math.floor(rnd() * 899999999));
    const hostname = `${site}-${kind}-${String(i % 1000).padStart(3, "0")}`;
    names.set(id, hostname);
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
  const status = healthFleet(org, names, now);
  // One agent claims to be a Tether host that has no health yet (a team member must confirm it); the
  // first linked host was confirmed by a member, the others were enrolled with tokens issued for their ID.
  const peerIds = new Set(peers.map((p) => p.id));
  const claimed = org.find((d, j) => j % 3 !== 0 && peerIds.has(d.id));
  if (claimed) status.push({ ...summary(4, "agent-only-4", null, "FRONT-DESK-AGENT", now), claimed_id: claimed.id });
  // An agent that claims a host whose health agent is linked already: Link stays refused.
  if (status[2]?.tether_id) status.push({ ...summary(1, "agent-only-5", null, "IMPOSTER-PC", now), claimed_id: status[2].tether_id });
  // A second relay where another device is linked to the second host's ID: a link conflict.
  const second: Summary[] = status[1]?.tether_id ? [summary(5, "agent-dup-1", status[1].tether_id, "SHADOW-PC", now)] : [];
  const all = () => [...status, ...second];
  const meta = new Map<string, Meta>(all().map((s, k) => [s.uuid, { enrolled: now - 40 * 86400 + k * 600, linked_by: null, linked_at: s.tether_id ? now - 40 * 86400 + k * 600 : null, no_claim: false }]));
  if (status[0]?.tether_id) Object.assign(meta.get(status[0].uuid)!, { linked_by: "Anna (Helpdesk)", linked_at: now - 5 * 86400 });
  for (const s of all()) s.linked_at = meta.get(s.uuid)!.linked_at;
  const find = (uuid: string) => all().find((x) => x.uuid === uuid) ?? fail("unknown device");
  return {
    peers,
    org,
    online: new Set(online),
    store: JSON.stringify(store),
    status,
    second,
    report: (uuid: string) => healthReport(all(), meta, uuid, now),
    sbom: (uuid: string) => sbomText(find(uuid).uuid, find(uuid).sbom?.at ?? now),
    link(uuid: string, id: string) {
      const s = find(uuid);
      if (s.claimed_id !== id) fail("Refused by the server: tether_id is not the one this device claims");
      Object.assign(s, { tether_id: id, claimed_id: null, linked_at: Math.floor(Date.now() / 1000) });
      Object.assign(meta.get(uuid)!, { linked_by: "Anna (Helpdesk)", linked_at: s.linked_at, no_claim: false });
    },
    unlink(uuid: string) {
      // The link and the claim go; the agent claims nothing until a member allows it.
      Object.assign(find(uuid), { tether_id: null, claimed_id: null, linked_at: null, no_claim: true });
      Object.assign(meta.get(uuid)!, { linked_by: null, linked_at: null, no_claim: true });
    },
    allowClaim(uuid: string) {
      find(uuid).no_claim = false;
      meta.get(uuid)!.no_claim = false;
    },
    remove(uuid: string) {
      const s = find(uuid);
      for (const l of [status, second]) if (l.includes(s)) l.splice(l.indexOf(s), 1);
    },
  };
}

type Meta = { enrolled: number; linked_by: string | null; linked_at: number | null; no_claim: boolean };
const fail = (e: string): never => {
  throw e;
};

// Health (team_status / team_status_host). No rnd() here: the fleet above stays as it was.
type Summary = Omit<StatusSummary, "relay">;
const iso = (t: number) => new Date(t * 1000).toISOString().replace(/\.\d+Z$/, "Z");
const T = (en: string, de: string) => ({ en, de });
const CHECKS: Record<string, { group: string; title: Text2; hint?: Text2; value?: string }> = {
  "app/db-service": { group: "Application", title: T("Database service", "Datenbankdienst"), hint: T("The database service is not running. Start it and check the event log.", "Der Datenbankdienst läuft nicht. Dienst starten und Ereignisprotokoll prüfen."), value: "stopped" },
  "app/web-service": { group: "Application", title: T("Web service", "Webdienst") },
  "backup/age": { group: "Backup", title: T("Backup age", "Alter der Sicherung"), hint: T("The last backup is older than two days. Check the backup job.", "Die letzte Sicherung ist älter als zwei Tage. Sicherungsauftrag prüfen."), value: "3 d" },
  "cert/expiry": { group: "Security", title: T("Certificate expiry", "Ablauf des Zertifikats"), hint: T("No certificate is configured for this check.", "Für diese Prüfung ist kein Zertifikat eingerichtet.") },
  "sys/disk": { group: "System", title: T("Free disk space", "Freier Speicherplatz"), value: "112 GB" },
  "sys/time": { group: "System", title: T("Time synchronisation", "Zeitsynchronisation") },
};
const STATES = ["ok", "ok", "warn", "crit", "ok", "setup"] as const;

function summary(k: number, uuid: string, tether_id: string | null, host: string, now: number): Summary {
  const st = STATES[k % 6];
  const stale = k % 13 === 0;
  const faults: StatusFault[] =
    st === "crit"
      ? [
          { id: "app/db-service", state: "crit", since: iso(now - 4 * 86400 - k * 60) },
          { id: "backup/age", state: "warn", since: iso(now - 2 * 86400) },
        ]
      : st === "warn"
        ? [{ id: "backup/age", state: "warn", since: iso(now - 2 * 86400 - k * 60) }]
        : st === "setup"
          ? [{ id: "cert/expiry", state: "unknown" }]
          : [];
  const server = host.includes("SRV");
  const products: StatusProduct[] = [
    { name: server ? "Records Server" : "Records Client", version: `26.${3 + (k % 2)}.${k % 4}`, role: server ? "server" : "client" },
    ...(k % 3 === 0 ? [{ name: "Imaging", version: "3.1" }] : []),
    ...(k % 4 === 1 ? [{ name: "Lab Link", version: "1.21" }] : []),
  ];
  const bad = (s: string) => faults.filter((f) => f.state === s).length;
  return {
    uuid,
    tether_id,
    host,
    agent: k % 2 ? "0.1.1" : "0.1.0",
    customer: k % 4 === 3 ? null : k % 4 === 2 ? "4712" : "4711",
    overall: st === "setup" ? "unknown" : st,
    counts: { ok: 12 - faults.length, warn: bad("warn"), crit: bad("crit"), unknown: bad("unknown"), setup: bad("unknown") },
    products,
    faults,
    last_seen: stale ? now - 4 * 3600 : now - (k % 5) * 60,
    next: 300,
    stale,
  };
}

function healthFleet(org: { uuid: string; id: string }[], names: Map<string, string>, now: number): Summary[] {
  const out = org.filter((_, j) => j % 3 === 0).map((d, k) => summary(k, d.uuid, d.id, names.get(d.id) ?? d.id, now));
  // Agent-only machines: no Tether installed.
  ["LAB-ANALYZER-01", "SCAN-STATION-02", "NAS-BACKUP-01"].forEach((host, i) => out.push({ ...summary([3, 1, 2][i], `agent-only-${i + 1}`, null, host, now), sbom: { at: now - 3600 * (i + 1), sha: `0123456789abcde${i}` } }));
  return out;
}

// Software lists (team_status_sbom): made-up programs (26, and 200 more on agent-only-1). agent-only-1 has a list, agent-only-2 is a list the relay
// dropped since the summary (404), agent-only-3 holds JSON that is not a software list.
const PROGRAMS: [string, string, string][] = [
  ["Aster Notes", "4.2.1", "Aster Labs"], ["Birchwood Reader", "11.0.3", "Birchwood Software"], ["Cobalt Archive Tool", "7.15", "Cobalt Systems"], ["Dunmore Backup Agent", "3.8.0", "Dunmore Data"],
  ["Elmstead Runtime", "17.0.9", "Elmstead Foundation"], ["Fenwick Spreadsheet", "2024.2", "Fenwick Office"], ["Garnet Browser", "126.0.2", "Garnet Project"], ["Hollin PDF Viewer", "9.1.4", "Hollin Documents"],
  ["Ironwood Terminal", "1.4.0", "Ironwood Tools"], ["Juniper Mail", "102.9.1", "Juniper Project"], ["Kestrel Remote Helper", "2.0.12", "Kestrel Networks"], ["Larkfield Codec Pack", "5.3", "Larkfield Media"],
  ["Meadow Text Editor", "8.6.2", "Meadow Software"], ["Northgate VPN Client", "6.1.0", "Northgate Security"], ["Oakham Image Editor", "3.9.7", "Oakham Graphics"], ["Pennant Chat", "1.88.0", "Pennant Communications"],
  ["Quarry Database Browser", "5.2.4", "Quarry Data"], ["Ravenscar Antivirus", "21.4", "Ravenscar Security"], ["Stonebridge Print Driver", "2.7.1", "Stonebridge Devices"], ["Thistle Video Player", "3.0.5", "Thistle Media"],
  ["Underhill Archive Manager", "1.12", "Underhill Tools"], ["Vale Calendar", "4.4.0", "Vale Software"], ["Wrenfield Scanner Service", "8.1.0", "Wrenfield Devices"], ["Yarrow Font Library", "2.2", "Yarrow Type"],
  ["Zephyr Screenshot Tool", "6.0.3", "Zephyr Utilities"],
];
function sbomText(uuid: string, at: number): string {
  if (uuid === "agent-only-2") return fail("none: No software list on the server.");
  if (uuid === "agent-only-3") return JSON.stringify({ hello: "world", components: "none" });
  const prop = (name: string, value: string) => ({ name, value });
  return JSON.stringify({
    bomFormat: "CycloneDX",
    specVersion: "1.6",
    serialNumber: "urn:uuid:7c1f6b5e-3f0a-4d8e-9a55-0c2d9a1b7e01",
    version: 4,
    metadata: {
      timestamp: iso(at),
      tools: { components: [{ type: "application", name: "tether-monitor", version: "0.1.2" }] },
      component: {
        type: "device",
        name: "LAB-ANALYZER-01",
        properties: [prop("tether:hw.manufacturer", "Example Devices"), prop("tether:hw.model", "Analyzer Station 7010"), prop("tether:hw.serial", "7XK2LAB0001"), prop("tether:hw.cpu", "Example Core i5-12500"), prop("tether:hw.memory_bytes", String(16 * 2 ** 30)), prop("tether:truncated", "12")],
      },
    },
    components: [
      { type: "operating-system", name: "Windows 11 Pro", version: "10.0.22631" },
      ...PROGRAMS.map(([name, version, publisher]) => ({ type: "application", name, version, publisher })),
      // More than 200 programs: the table draws 200 until "Show all".
      ...Array.from({ length: 200 }, (_, i) => ({ type: "application", name: `Sample Tool ${String(i + 1).padStart(3, "0")}`, version: `1.${i}`, publisher: "Sample Works" })),
      { type: "application", name: "Records Server", version: "26.3.2", publisher: "Example Health Software", properties: [prop("tether:product", "true"), prop("tether:role", "server")] },
    ],
  });
}

function healthReport(status: Summary[], meta: Map<string, Meta>, uuid: string, now: number): StatusHostReply {
  const s = status.find((x) => x.uuid === uuid);
  if (!s) throw "unsupported: The server does not offer team access yet.";
  const link = { claimed_id: s.claimed_id ?? null, ...meta.get(uuid)! };
  const checks: ReportCheck[] = Object.entries(CHECKS).map(([id, c]) => {
    const f = s.faults.find((x) => x.id === id);
    const setup = id === "cert/expiry";
    return { id, group: c.group, state: setup ? "unknown" : (f?.state ?? "ok"), ...(f?.since ? { since: f.since } : {}), ...(c.value && (f || !c.hint) ? { value: c.value } : {}), ...(setup ? { setup: true } : {}), title: c.title, ...(c.hint ? { hint: c.hint } : {}) };
  });
  const steps = [["backup/age", "ok", "warn"], ["backup/age", "warn", "ok"], ["app/db-service", "ok", "crit"], ["app/db-service", "crit", "ok"], ["sys/disk", "ok", "warn"], ["sys/disk", "warn", "ok"]] as const;
  const history: StatusChange[] = steps.map(([id, from, to], i) => {
    const at = now - (6 - i) * 2 * 86400;
    return { id, from, to, time: iso(at), at };
  });
  const { last_seen, next, stale, tether_id, claimed_id: _c, no_claim: _n, customer, ...rest } = s;
  // A report may leave out products, changes and body (the relay stores it as sent).
  if (uuid === "agent-only-3") {
    const { products: _p, ...bare } = rest;
    return { uuid, tether_id, ...link, customer, last_seen, next, stale, report: { v: 1, ...bare, next, seq: last_seen * 1000, time: iso(last_seen), packs: {} }, history: [] };
  }
  return {
    uuid,
    tether_id,
    ...link,
    customer,
    last_seen,
    next,
    stale,
    report: { v: 1, ...rest, next, ...(tether_id ? { tether_id } : {}), seq: last_seen * 1000, time: iso(last_seen), packs: { base: 3, records: 2 }, changes: [], body: { checks } },
    history,
  };
}
