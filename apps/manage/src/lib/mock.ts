// Browser-only stand-in for the Tauri backend so the UI can be previewed with `npm run dev`.
// Sample data, no crypto, nothing leaves the page.
import type { Bundle, Feature } from "./ipc";

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
  f("video.quality", "video", "Default image quality", "choice", "balanced", ["technician"], ["best", "balanced", "low"]),
  f("recording.mode", "recording", "Session recording", "choice", "on-request", ["technician"], ["off", "on-request", "always"]),
  f("update.channel", "updates", "Update channel", "text", "stable"),
  f("customer.title", "customer", "Window title / company name", "text", "Tether Support", ["customer"]),
];

let unlocked = false;
let draft: Bundle = {
  serial: 41, issued_at: 1793700000,
  profiles: { technician: { "permission.clipboard": { state: "off" }, "video.quality": { state: "on", value: "best" } } },
  groups: { "front-desk": { profile: "technician", overrides: { "permission.file-transfer": { state: "hidden" } } } },
  assignments: { "a1b2c3d4-0001": "front-desk" },
};
const now = Math.floor(Date.now() / 1000);
const devices = [
  { uuid: "a1b2c3d4-0001", id: "482 019 337", profile: "technician", version: "0.3.0", os: "windows 10.0.26300", serial: 41, ip: "84.12.9.20", last_seen: now - 120 },
  { uuid: "a1b2c3d4-0002", id: "771 204 118", profile: "technician", version: "0.2.1", os: "windows 10.0.22631", serial: 40, ip: "84.12.9.21", last_seen: now - 7200 },
  { uuid: "c9f0e1aa-7731", id: "305 662 901", profile: "customer", version: "0.3.0", os: "windows 10.0.19045", serial: 41, ip: "91.4.77.3", last_seen: now - 400 },
];

const admin: Record<string, unknown> = {
  "/api/admin/status": { version: "0.3.0", disk_free: 18_400_000_000, devices: 3, policy_serial: 41, uptime: 1_296_000 },
  "/api/admin/devices": devices,
  "/api/admin/releases": [
    { channel: "stable", path: "technician/windows-x86_64/Tether_0.3.0_x64-setup.exe", size: 14_200_000, modified: now - 86400 * 3 },
    { channel: "stable", path: "technician/windows-x86_64/Tether_0.3.0_x64-setup.exe.sig", size: 420, modified: now - 86400 * 3 },
    { channel: "stable", path: "technician/windows-x86_64/latest.json", size: 610, modified: now - 86400 * 3 },
    { channel: "stable", path: "customer/windows-x86_64/TetherQS-0.3.0.exe", size: 9_800_000, modified: now - 86400 * 3 },
  ],
  "/api/admin/diag": [
    { ticket: "K7Q2M9XD", note: "Black screen after login, Dr. Weber", created: now - 3600, uploaded: now - 3000, size: 182_000 },
    { ticket: "P4RT8ZNA", note: "Audio missing", created: now - 600, uploaded: null, size: null },
  ],
  "/api/admin/diag-ticket": { ticket: "W3HX9QEL", expires: now + 86400 },
};

function resolve(b: Bundle, profile: string, device: string) {
  const values: Record<string, string> = {};
  const locked: string[] = [];
  const hidden: string[] = [];
  FEATURES.filter((x) => x.profiles.includes(profile)).forEach((x) => (values[x.id] = x.default));
  const g = b.assignments[device];
  const layers = [b.profiles[profile] ?? {}, g && b.groups[g]?.profile === profile ? b.groups[g].overrides : {}];
  for (const l of layers)
    for (const [id, r] of Object.entries(l)) {
      const ft = FEATURES.find((x) => x.id === id);
      if (!ft || !(id in values)) continue;
      values[id] = r.state === "off" || r.state === "hidden" ? ft.off_value : r.value ?? (ft.kind === "bool" ? "Y" : ft.default);
      if (r.state !== "on") locked.push(id);
      if (r.state === "hidden") hidden.push(id);
    }
  return { profile, serial: b.serial, group: g ?? null, values, locked, hidden, sources: ["defaults", "profile"] };
}

export async function mockInvoke<T>(cmd: string, a: any = {}): Promise<T> {
  await new Promise((r) => setTimeout(r, 120));
  const out: Record<string, () => unknown> = {
    state: () => ({
      exists: true, unlocked,
      admin_pub: unlocked ? "q3Jm0yK0b0gk2sVQ1Q3c8b6m1Jf1n1w2C6o5tB1yZ2A=" : null,
      updater_pub: unlocked ? "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDRGM0E5QjFDMkQ3RTVBNjEKUldSaFduNHRISnM2VDJ5" : null,
      settings: { server: "https://tether-desk.duckdns.org", page_title: "Remote support", page_intro: "Download Tether to get help from our support team." },
      data_dir: "C:\\Users\\you\\AppData\\Roaming\\Tether-Manage",
    }),
    keys_unlock: () => (unlocked = true),
    keys_lock: () => (unlocked = false),
    admin: () => admin[a.path] ?? null,
    features: () => FEATURES,
    policy_draft: () => draft,
    policy_save_draft: () => (draft = a.bundle),
    policy_check: () => null,
    policy_preview: () => resolve(a.bundle, a.profile, a.device),
    policy_history: () => [41, 40, 39].map((s, i) => ({ serial: s, issued_at: now - i * 86400 * 4, server: true, local: i < 2 })),
    policy_publish: () => 42,
    page_render: () => `<html><body style="font-family:system-ui;padding:40px"><h1>${a.title}</h1><p>${a.intro}</p></body></html>`,
    diag_open: () => ({
      size: 182_000,
      files: [{ name: "diagnostics.json", size: 2100 }, { name: "logs/tether.log", size: 160_000 }, { name: "logs/core.log", size: 21_000 }],
      diagnostics: JSON.stringify({ app: "0.3.0", os: "windows 10.0.26300", route: "443/direct", policy_serial: 41 }, null, 2),
    }),
  };
  if (!(cmd in out)) return null as T;
  return out[cmd]() as T;
}
