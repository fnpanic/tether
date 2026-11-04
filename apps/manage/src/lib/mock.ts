// Browser-only stand-in for the Tauri backend so the UI can be previewed with `npm run dev`.
// Sample data, no crypto, nothing leaves the page. Two relays; the second is one policy serial and
// one release behind, and the policy names a third relay that is not configured.
import type { Bundle, Feature, Layer, Rule } from "./ipc";

const f = (id: string, area: string, en: string, kind: Feature["kind"], def: string, profiles = ["technician", "customer"], choices: string[] = []): Feature => ({
  id, area, en, de: en, kind, default: def, choices, off_value: kind === "bool" ? "N" : choices[0] ?? "", option: null, profiles,
});
const FEATURES: Feature[] = [
  f("permission.control", "permissions", "Allow mouse and keyboard control", "bool", "Y"),
  f("permission.clipboard", "permissions", "Allow clipboard sync", "bool", "Y"),
  f("permission.file-transfer", "permissions", "Allow file transfer", "bool", "Y"),
  f("security.approve-mode", "security", "How incoming sessions are approved", "choice", "both", undefined, ["both", "click", "password"]),
  f("security.unattended", "security", "Allow unattended access (background service)", "bool", "Y", ["technician"]),
  f("connect.reconnect-minutes", "connections", "Keep reconnecting after a connection loss (minutes)", "number", "10"),
  f("servers.list", "connections", "Additional relay servers (host names or wss:// URLs, comma-separated)", "text", ""),
  f("video.quality", "video", "Default image quality", "choice", "balanced", ["technician"], ["best", "balanced", "low"]),
  f("recording.mode", "recording", "Session recording", "choice", "on-request", ["technician"], ["off", "on-request", "always"]),
  f("update.channel", "updates", "Update channel", "text", "stable"),
  f("policy.refresh-minutes", "policy", "Check for policy changes every (minutes)", "number", "15"),
  f("policy.fast-apply", "policy", "Fast apply (check every minute)", "bool", "N"),
  f("customer.title", "customer", "Window title / company name", "text", "Tether Support", ["customer"]),
  f("customer.close-after-session", "customer", "Quit when the session ends", "bool", "N", ["customer"]),
  f("customer.elevate", "customer", "Offer \"Elevate\" (admin access for UAC prompts)", "bool", "Y", ["customer"]),
];

const R1 = "https://tether-desk.duckdns.org";
const R2 = "https://tether-desk2.duckdns.org";
let servers = [R1, R2];
let unlocked = false;
let liveBundle: Bundle = {
  serial: 4, issued_at: 1793700000,
  profiles: {
    technician: { "permission.clipboard": { state: "off" }, "video.quality": { state: "on", value: "best" }, "servers.list": { state: "locked", value: "tether-desk.duckdns.org, tether-desk2.duckdns.org, tether-desk3.duckdns.org" } },
  },
  groups: { "front-desk": { profile: "technician", overrides: { "permission.file-transfer": { state: "hidden" } } } },
  assignments: { "a1b2c3d4-0001": "front-desk" },
};
let draft: Bundle = JSON.parse(JSON.stringify(liveBundle));
draft.profiles.technician["recording.mode"] = { state: "locked", value: "always" };
draft.profiles.technician["permission.clipboard"] = { state: "on" };
draft.profiles.customer = { "security.approve-mode": { state: "locked", value: "click" } };
let relaySerial: Record<string, number> = { [R1]: 4, [R2]: 3 };

const now = Math.floor(Date.now() / 1000);
const today = (() => {
  const d = new Date();
  d.setHours(0, 0, 0, 0);
  return Math.floor(d.getTime() / 1000);
})();
const devices = [
  { uuid: "a1b2c3d4-0001", id: "482 019 337", profile: "technician", version: "0.4.0", os: "windows 10.0.26300", serial: 4, ip: "84.12.9.20", last_seen: now - 120 },
  { uuid: "a1b2c3d4-0002", id: "771 204 118", profile: "technician", version: "0.3.1", os: "windows 10.0.22631", serial: 3, ip: "84.12.9.21", last_seen: Math.max(today + 60, now - 7200) },
  { uuid: "a1b2c3d4-0003", id: "120 554 902", profile: "technician", version: "0.4.0", os: "macos 15.1", serial: 4, ip: "84.12.9.40", last_seen: now - 900 },
  { uuid: "c9f0e1aa-7731", id: "305 662 901", profile: "customer", version: "0.4.0", os: "windows 10.0.19045", serial: 4, ip: "91.4.77.3", last_seen: now - 400 },
  { uuid: "c9f0e1aa-7732", id: "305 662 950", profile: "customer", version: "0.3.0", os: "windows 10.0.19045", serial: 2, ip: "91.4.77.9", last_seen: today - 86400 * 3 },
  { uuid: "d0d0e1aa-0101", id: "611 093 448", profile: "customer", version: "0.4.0", os: "macos 14.6", serial: 3, ip: "77.20.1.5", last_seen: today - 3600 },
];

const admin: Record<string, unknown> = {
  "/api/admin/devices": devices,
  "/api/admin/releases": [
    { channel: "stable", path: "technician/windows-x86_64/Tether_0.4.0_x64-setup.exe", size: 14_200_000, modified: now - 86400 * 3 },
    { channel: "stable", path: "technician/windows-x86_64/Tether_0.4.0_x64-setup.exe.sig", size: 420, modified: now - 86400 * 3 },
    { channel: "stable", path: "technician/windows-x86_64/latest.json", size: 610, modified: now - 86400 * 3 },
    { channel: "stable", path: "customer/windows-x86_64/TetherQS-0.4.0.exe", size: 9_800_000, modified: now - 86400 * 3 },
  ],
  "/api/admin/diag": [
    { ticket: "K7Q2M9XD", note: "Black screen after login, Dr. Weber", created: now - 3600, uploaded: now - 3000, size: 182_000 },
    { ticket: "P4RT8ZNA", note: "Audio missing", created: now - 600, uploaded: null, size: null },
  ],
  "/api/admin/diag-ticket": { ticket: "W3HX9QEL", expires: now + 86400 },
};

function resolve(b: Bundle | null, profile: string, device: string) {
  const values: Record<string, string> = {};
  const locked: string[] = [];
  const hidden: string[] = [];
  FEATURES.filter((x) => x.profiles.includes(profile)).forEach((x) => (values[x.id] = x.default));
  const g = b?.assignments[device];
  const layers = [b?.profiles[profile] ?? {}, g && b?.groups[g]?.profile === profile ? b.groups[g].overrides : {}];
  for (const l of layers)
    for (const [id, r] of Object.entries(l)) {
      const ft = FEATURES.find((x) => x.id === id);
      if (!ft || !(id in values)) continue;
      values[id] = r.state === "off" || r.state === "hidden" ? ft.off_value : r.value ?? (ft.kind === "bool" ? "Y" : ft.default);
      if (r.state !== "on") locked.push(id);
      if (r.state === "hidden") hidden.push(id);
    }
  return { profile, serial: b?.serial ?? 0, group: g ?? null, values, locked, hidden, sources: ["defaults", "profile"] };
}

const words = (id: string, r?: Rule) => {
  if (!r) return "default";
  const ft = FEATURES.find((x) => x.id === id);
  const v = r.value ?? (ft?.kind === "bool" ? "Y" : ft?.default ?? "");
  const s = v === "Y" ? "on" : v === "N" ? "off" : v;
  return r.state === "on" ? (ft?.kind === "bool" && v === "Y" ? "on" : `on (${s}, user may change)`) : r.state === "locked" ? (ft?.kind === "bool" ? `locked ${s}` : `locked to ${s}`) : r.state;
};
/** Same idea as live::changes in Rust, enough for previews. */
function diff(from: Bundle | null, to: Bundle) {
  const out: { scope: string; label: string | null; from: string | null; to: string | null; text: string }[] = [];
  const layer = (scope: string, a: Layer = {}, b: Layer = {}) => {
    for (const id of [...new Set([...Object.keys(a), ...Object.keys(b)])].sort()) {
      if (JSON.stringify(a[id]) === JSON.stringify(b[id])) continue;
      const label = FEATURES.find((x) => x.id === id)?.en ?? id;
      out.push({ scope, label, from: words(id, a[id]), to: words(id, b[id]), text: `${scope}: ${label}` });
    }
  };
  for (const p of ["technician", "customer"]) layer(p === "technician" ? "Technician profile" : "Customer profile", from?.profiles[p], to.profiles[p]);
  for (const g of new Set([...Object.keys(from?.groups ?? {}), ...Object.keys(to.groups)])) {
    if (!from?.groups[g]) out.push({ scope: `Group ${g}`, label: null, from: null, to: null, text: `Group ${g} created for ${to.groups[g].profile} devices` });
    if (!to.groups[g]) out.push({ scope: `Group ${g}`, label: null, from: null, to: null, text: `Group ${g} deleted` });
    else layer(`Group ${g}`, from?.groups[g]?.overrides, to.groups[g].overrides);
  }
  for (const d of new Set([...Object.keys(from?.assignments ?? {}), ...Object.keys(to.assignments)])) {
    const [a, b] = [from?.assignments[d], to.assignments[d]];
    if (a !== b) out.push({ scope: "Devices", label: null, from: a ?? null, to: b ?? null, text: b ? `Device ${d.slice(0, 8)} ${a ? `moved from group ${a} to ${b}` : `added to group ${b}`}` : `Device ${d.slice(0, 8)} removed from group ${a}` });
  }
  return out;
}

const status = (sv: string) => ({ version: "0.4.0", disk_free: sv === R1 ? 18_400_000_000 : 1_600_000_000, devices: sv === R1 ? 5 : 2, policy_serial: relaySerial[sv] ?? 0, uptime: sv === R1 ? 1_296_000 : 7_400 });
const ok = (server: string, detail: unknown = null) => ({ server, ok: true, detail });

const logo = (() => {
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const g = c.getContext("2d")!;
  g.fillStyle = "#c8102e";
  g.fillRect(0, 0, 64, 64);
  g.fillStyle = "#fff";
  g.font = "bold 30px system-ui";
  g.fillText("AC", 12, 43);
  return c.toDataURL("image/png").split(",")[1];
})();
let custom: any[] = [
  {
    id: "k3v9q2m7x1c8w4za", name: "ACME Dental", token: "Qm9vZ2llLXRva2VuLWZvci1wcmV2aWV3LW9ubHktMDAx", expires: now + 86400 * 23, channel: "stable", created: now - 86400 * 7, downloads: 4, last_download: now - 7200,
    local: { id: "k3v9q2m7x1c8w4za", name: "ACME Dental", accent: "#c8102e", welcome: "Welcome to ACME Dental support. Read this ID to Anna.", logo_png: logo, layer: { "security.approve-mode": { state: "locked", value: "click" }, "customer.close-after-session": { state: "locked" } }, issued_at: now - 86400 * 7 },
  },
  { id: "p0x7n2", name: "Praxis Weber", token: "UHJheGlzLVdlYmVyLXByZXZpZXctdG9rZW4tMDAwMDAy", expires: now + 86400 * 4, channel: "stable", created: now - 86400 * 26, downloads: 1, last_download: now - 86400 * 20, local: null },
];

export async function mockInvoke<T>(cmd: string, a: any = {}): Promise<T> {
  await new Promise((r) => setTimeout(r, 120));
  const out: Record<string, () => unknown> = {
    state: () => ({
      exists: true, unlocked,
      admin_pub: unlocked ? "q3Jm0yK0b0gk2sVQ1Q3c8b6m1Jf1n1w2C6o5tB1yZ2A=" : null,
      updater_pub: unlocked ? "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRGM0E5QjFDMkQ3RTVBNjEKUldSaFduNHRISnM2VDJ5" : null,
      settings: { server: servers.join(", "), release_folder: "C:/install/remote-support/dist", page_title: "Remote support", page_intro: "Download Tether to get help from our support team." },
      servers,
      data_dir: "C:\\Users\\you\\AppData\\Roaming\\Tether-Manage",
    }),
    keys_unlock: () => (unlocked = true),
    keys_lock: () => (unlocked = false),
    settings_set: () => (servers = String(a.settings.server).split(",").map((s: string) => s.trim()).filter(Boolean)),
    admin: () => (a.path === "/api/admin/status" ? status(R1) : admin[a.path] ?? null),
    relays_check: () => servers.map((s) => ok(s, status(s))),
    devices_all: () => ({ devices, failed: [] }),
    features: () => FEATURES,
    policy_draft: () => draft,
    policy_save_draft: () => (draft = a.bundle),
    policy_check: () => null,
    policy_preview: () => resolve(a.bundle, a.profile, a.device),
    policy_diff: () => diff(a.from, a.to),
    policy_live: () => {
      const eff = (p: string) => {
        const e = resolve(liveBundle, p, "");
        return Object.entries(e.values).map(([id, value]) => ({ id, value, locked: e.locked.includes(id), hidden: e.hidden.includes(id), source: liveBundle.profiles[p]?.[id] ? "profile" : "default" }));
      };
      return {
        relays: servers.map((s) => ({ server: s, reachable: true, error: null, serial: relaySerial[s] ?? null, issued_at: liveBundle.issued_at - (relaySerial[s] === liveBundle.serial ? 0 : 86400), verified: true, note: null })),
        serial: liveBundle.serial, issued_at: liveBundle.issued_at,
        effective: { technician: eff("technician"), customer: eff("customer") },
        unconfigured: ["tether-desk3.duckdns.org"].filter((h) => !servers.includes(`https://${h}`)),
        bundle: liveBundle,
      };
    },
    policy_publish: () => {
      const serial = liveBundle.serial + 1;
      liveBundle = { ...a.bundle, serial, issued_at: now };
      servers.forEach((s) => (relaySerial[s] = serial));
      return { serial, relays: servers.map((s) => ok(s, { serial })) };
    },
    policy_sync: () => servers.map((s) => {
      const already = relaySerial[s] === liveBundle.serial;
      relaySerial[s] = liveBundle.serial;
      return ok(s, { serial: liveBundle.serial, already });
    }),
    policy_history: () => [4, 3, 2].map((s, i) => ({ serial: s, issued_at: now - i * 86400 * 4, server: true, local: i < 2 })),
    policy_local: () => liveBundle,
    releases_overview: () => ({
      errors: [],
      rows: [
        { channel: "stable", kind: "technician", platform: "windows-x86_64", latest: "0.4.0", relays: [{ server: R1, published: "0.4.0", has_latest: true, error: null }, { server: R2, published: "0.3.1", has_latest: false, error: null }] },
        { channel: "stable", kind: "technician", platform: "darwin-universal", latest: "0.4.0", relays: [{ server: R1, published: "0.4.0", has_latest: true, error: null }, { server: R2, published: null, has_latest: false, error: null }] },
        { channel: "stable", kind: "customer", platform: "windows-x86_64", latest: "0.4.0", relays: [{ server: R1, published: "0.4.0", has_latest: true, error: null }, { server: R2, published: "0.4.0", has_latest: true, error: null }] },
        { channel: "beta", kind: "technician", platform: "windows-x86_64", latest: "0.4.1-beta", relays: [{ server: R1, published: "0.4.0", has_latest: true, error: null }, { server: R2, published: "0.4.0", has_latest: false, error: null }] },
      ].map((r) => ({ ...r, relays: r.relays.filter((x) => servers.includes(x.server)) })),
    }),
    release_sync: () => servers.map((s) => (s === R1 ? ok(s, { already: true }) : ok(s, { url: `${s}/download/…` }))),
    release_scan: () => [
      { kind: "customer", platform: "darwin-universal", version: "0.5.1", file: "dist/macos/TetherQS-0.5.1-universal.zip", published: "0.4.0", newer: true, problem: null },
      { kind: "customer", platform: "windows-x86_64", version: "0.5.1", file: "dist/quick/TetherQS-0.5.1.exe", published: "0.5.1", newer: false, problem: null },
      { kind: "technician", platform: "darwin-universal", version: "0.5.1", file: "dist/macos/Tether_0.5.1_universal.app.tar.gz", published: "0.4.0", newer: true, problem: null },
      { kind: "technician", platform: "windows-x86_64", version: "0.5.1", file: "dist/windows/Tether_0.5.1_x64-setup.exe", published: "0.4.0", newer: true, problem: null },
    ],
    release_publish: () => servers.map((s) => ok(s, { url: `${s}/download/…` })),
    page_render: async () => (await import("./mock-page.html?raw")).default,
    page_publish: () => servers.map((s) => ok(s)),
    custom_list: () => custom.map((c) => ({ ...c, link: `${R1}/q/${c.token}`, status: c.expires > now ? "active" : "expired" })),
    custom_save: () => {
      const id = a.args.id ?? Math.random().toString(36).slice(2, 18);
      const token = "TmV3LXByZXZpZXctdG9rZW4tZm9yLXRoZS1tb2NrLTAx";
      const row = { id, name: a.args.name, token, expires: now + a.args.days * 86400, channel: a.args.channel, created: now, downloads: 0, last_download: null, local: { ...a.args, id, issued_at: now } };
      custom = [...custom.filter((c) => c.id !== id), row];
      return { id, link: `${R1}/q/${token}`, relays: servers.map((s) => ok(s)) };
    },
    custom_delete: () => {
      custom = custom.filter((c) => c.id !== a.id);
      return servers.map((s) => ok(s));
    },
    diag_open: () => ({
      size: 182_000,
      files: [{ name: "diagnostics.json", size: 2100 }, { name: "logs/tether.log", size: 160_000 }, { name: "logs/core.log", size: 21_000 }],
      diagnostics: JSON.stringify({ app: "0.4.0", os: "windows 10.0.26300", route: "443/direct", policy_serial: 4 }, null, 2),
    }),
  };
  if (!(cmd in out)) return null as T;
  return (await out[cmd]()) as T;
}
