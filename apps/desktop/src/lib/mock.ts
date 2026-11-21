// In-browser stand-in for the Rust backend, used by `npm run dev` outside Tauri and UI screenshots.
import type { Evt } from "./ipc";

type Cb = (e: Evt) => void;
const listeners: Record<string, Cb[]> = { session: [], cm: [] };

const state = {
  password: "k7x9q2",
  options: {} as Record<string, string>,
  peers: [
    { id: "184392571", hostname: "Design-Studio", username: "maria", platform: "Mac OS" },
    { id: "902114538", hostname: "FINANCE-PC-07", username: "j.berg", platform: "Windows" },
    { id: "551870034", hostname: "build-runner", username: "ci", platform: "Linux" },
    { id: "327660915", hostname: "Reception", username: "front", platform: "Windows" },
  ],
};

// Advanced mode's mock fleet is its own chunk, like the page it feeds.
let fleetCache: Promise<import("./mock-fleet").Fleet> | null = null;
const fleet = () => (fleetCache ??= import("./mock-fleet").then((m) => m.fleet()));
const team = { rev: 0, doc: null as unknown, by: "", at: 0 };

function emit(event: string, name: string, payload: unknown) {
  for (const cb of listeners[event] ?? []) cb({ name, payload });
}

function rgbaFrame(w: number, h: number): ArrayBuffer {
  const buf = new ArrayBuffer(16 + w * h * 4);
  const v = new DataView(buf);
  v.setUint8(0, 1); // PKT_RGBA
  v.setUint32(4, w, true);
  v.setUint32(8, h, true);
  new Uint8Array(buf, 16).fill(90);
  return buf;
}

export const mock = {
  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    const a = args ?? {};
    // UI tests set `window.__mockCalls = []` to see what the page asked the backend for.
    (globalThis as { __mockCalls?: unknown[] }).__mockCalls?.push({ cmd, args: a });
    switch (cmd) {
      case "status":
        return { id: "731204986", password: state.password, status: 1, key_confirmed: true, installed: false, version: "0.1.0" } as T;
      case "refresh_password":
        state.password = Math.random().toString(36).slice(2, 8);
        return undefined as T;
      case "recent_peers":
        // `?hosts=N` also fills the recent list (Home search shows from 5 devices); `?recent=0` empties it.
        if (new URLSearchParams(location.search).get("recent") === "0") return [] as T;
        return (new URLSearchParams(location.search).has("hosts") ? [...state.peers, ...(await fleet()).peers.slice(0, 40)] : state.peers) as T;
      case "remove_peer":
        state.peers = state.peers.filter((p) => p.id !== a.id);
        return undefined as T;
      case "tfa_enabled":
        return (state.options["2fa"] === "on") as T;
      case "tfa_begin":
        return "otpauth://totp/Tether:731204986?secret=JBSWY3DPEHPK3PXP&issuer=Tether" as T;
      case "tfa_confirm":
        if (a.code !== "123456") throw "That code is not valid. Check the time on your phone and try again.";
        state.options["2fa"] = "on";
        return undefined as T;
      case "tfa_disable":
        state.options["2fa"] = "";
        return undefined as T;
      case "get_option":
        return (state.options[a.key as string] ?? "") as T;
      case "set_option":
        state.options[a.key as string] = a.value as string;
        return undefined as T;
      case "set_permanent_password":
        if (String(a.password).length < 12) throw "Use at least 12 characters.";
        return undefined as T;
      case "open_session":
        window.open(`#/session/mock?peer=${a.peer}&conn=${a.conn ?? "default"}`, "_blank");
        return "mock" as T;
      case "session_start":
        const who = state.peers.find((p) => p.id === a.peer) ?? (a.peer ? (await fleet()).peers.find((p) => p.id === a.peer) : undefined) ?? { hostname: String(a.peer ?? ""), username: "", platform: "Windows" };
        setTimeout(() => emit("session", "peer_info", { hostname: who.hostname, username: who.username, platform: who.platform, displays: [{ x: 0, y: 0, width: 2560, height: 1440 }, { x: 2560, y: 0, width: 1920, height: 1080 }], current_display: 0 }), 600);
        setTimeout(() => emit("session", "connection_type", { secured: true, direct: true, stream_type: "" }), 650);
        // One grey 16:9 picture (RGBA packet, see video.ts), so the stage and the toolbar show.
        setTimeout(() => (a.video as { onmessage?: (b: ArrayBuffer) => void } | null)?.onmessage?.(rgbaFrame(320, 180)), 700);
        setTimeout(() => emit("session", "quality", { fps: { 0: 60 }, delay: 14, target_bitrate: 3800, codec: "H264", speed: "4.2MB/s" }), 900);
        return undefined as T;
      case "recordings_list":
        return {
          folder: "C:\\Users\\demo\\Videos\\Tether",
          custom: false,
          locked: false,
          items: [
            { name: "2026-11-05_14-03-22_Design-Studio_184392571.webm", peer_id: "184392571", peer_name: "Design-Studio", started_ms: 1793973802000, duration_s: 725, size: 48_234_112, pending: false },
            { name: "2026-11-04_09-12-40_402118736.mp4", peer_id: "402118736", peer_name: "", started_ms: 1793869960000, duration_s: 94, size: 6_120_448, pending: true },
          ],
        } as T;
      case "cm_clients":
        // `?cm=1`: only the first (a request).
        return [{ id: 1, peer_id: "184392571", name: "maria@Design-Studio", authorized: false, keyboard: true, clipboard: true, audio: true, file: true, restart: true, recording: false, block_input: false, is_file_transfer: false },
          { id: 2, peer_id: "902114538", name: "j.berg@FINANCE-PC-07", authorized: true, keyboard: true, clipboard: true, audio: true, file: true, restart: true, recording: false, block_input: false, is_file_transfer: false }].slice(0, new URLSearchParams(location.search).get("cm") === "1" ? 1 : 2) as T;
      case "policy_state": {
        // `?ui=forced|off` in the page URL: policy ui.advanced for the mode tests.
        const ui = new URLSearchParams(location.search).get("ui");
        return (ui ? { profile: "technician", serial: 1, group: null, values: { "ui.advanced": ui }, locked: ["ui.advanced"], hidden: [], sources: ["profile"] } : undefined) as T;
      }
      case "main_window_fit":
        return undefined as T;
      case "hosts_peers":
        return [...state.peers.map((p, i) => ({ ...p, last: Math.floor(Date.now() / 1000) - i * 3600 })), ...(await fleet()).peers] as T;
      case "hosts_store_load":
        try {
          const f = await fleet();
          return (localStorage.getItem("tether.mock.hostsStore") ?? (f.peers.length ? f.store : "")) as T;
        } catch {
          return "" as T;
        }
      case "hosts_store_save":
        if (String(a.doc).length > 16 * 1024 * 1024) throw "The host list is too large to save (16 MB).";
        try {
          localStorage.setItem("tether.mock.hostsStore", String(a.doc));
        } catch {
          /* quota: keep in memory only */
        }
        return undefined as T;
      case "hosts_online": {
        await new Promise((r) => setTimeout(r, 120));
        const f = await fleet();
        return (a.ids as string[]).filter((id) => f.online.has(id) || id === "184392571" || id === "551870034") as T;
      }
      case "sessions_open":
        return [
          { key: "k1", peer: "184392571", conn: "default" },
          { key: "k2", peer: "551870034", conn: "file-transfer" },
        ] as T;
      case "session_focus_window":
        return true as T;
      case "team_identity":
        return { key: "q2o5n0d6m7m3l1k8p4g2b7r9x0c5v8z1a3s6d9f2g4h=", fingerprint: "9F3A-12B0-C4D1-77E2" } as T;
      case "team_me":
        if (new URLSearchParams(location.search).get("team") === "none") throw "not-member: This app is not in your organisation's team. Send its key to your administrator.";
        // `?team=ro`: a member without write permission.
        return { name: "Anna (Helpdesk)", write: new URLSearchParams(location.search).get("team") !== "ro", audit: true } as T;
      case "team_devices":
        return (await fleet()).org as T;
      case "team_tags_get":
        return { rev: team.rev, doc: team.doc, updated_by: team.by, updated_at: team.at } as T;
      case "team_tags_put":
        if (a.rev !== team.rev) return { ok: false, conflict: { rev: team.rev, doc: team.doc } } as T;
        team.rev++;
        team.doc = JSON.parse(String(a.doc));
        team.by = "Anna (Helpdesk)";
        team.at = Math.floor(Date.now() / 1000);
        return { ok: true, rev: team.rev } as T;
      case "team_audit": {
        const t = Math.floor(Date.now() / 1000);
        return [
          { ts: t - 3600, action: "login", id: a.device, peer_id: "731204986", peer_name: "anna", conn_type: "remote", ip: "198.51.100.7" },
          { ts: t - 2400, action: "disconnect", id: a.device },
          { ts: t - 86400 * 3, action: "login", id: a.device, peer_id: "402118736", peer_name: "ben", conn_type: "file_transfer", ip: "203.0.113.20" },
        ] as T;
      }
      case "session_keyboard":
        return { dir: "pc-mac", shortcuts: true, system_keys: true, alt_tab_remote: false, hook: true } as T;
      case "fs_home_dir":
        return "C:\\Users\\demo" as T;
      case "fs_local_dir":
        return { path: a.path, entries: [{ name: "Documents", type: 1, size: 0, modified: 1761900000 }, { name: "Downloads", type: 1, size: 0, modified: 1761900000 }, { name: "report-q3.pdf", type: 4, size: 2483021, modified: 1761900000 }, { name: "notes.txt", type: 4, size: 1204, modified: 1761900000 }] } as T;
      case "diag_info": {
        // Settings › Diagnostics as on a real install (long names, two relays, one route failing).
        const t = Math.floor(Date.now() / 1000);
        return {
          version: "0.5.27",
          id: "731204986",
          os: "Windows 10.0.26300 (Workstation)",
          policy: { serial: 12, group: "Reception desks Hamburg", sources: ["relay", "cache"] },
          relay: "relay-eu-west-1.support.example-company-name.com",
          proxy: "http://proxy.corp.example-company-name.internal:3128",
          dns: { host: "relay-eu-west-1.support.example-company-name.com", addrs: ["203.0.113.10", "2001:db8::10"], ms: 4 },
          routes: [
            { url: "wss://relay-eu-west-1.support.example-company-name.com/ws/id", via_proxy: true, ok: true, error: "", ms: 229, at: t - 60 },
            { url: "wss://relay-us-east-2.support.example-company-name.com/ws/id", via_proxy: false, ok: false, error: "connection timed out after 5 s (tcp connect to 203.0.113.9:443)", ms: 5000, at: t - 60 },
          ],
          errors: ["[2026-11-08 09:12:40] WARN relay-us-east-2.support.example-company-name.com: connection timed out after 5 s"],
          verboseUntil: 0,
          logDir: "C:\Users\demo\AppData\Roaming\Tether\log",
        } as T;
      }
      case "diag_upload":
        // The code K7Q2M9XD is accepted; any other gets the message for an unreachable relay.
        await new Promise((r) => setTimeout(r, 150));
        if (a.ticket !== "K7Q2M9XD") throw "Could not reach the support server. Try again later; if it keeps failing, ask your support contact for a new code.";
        return undefined as T;
      default:
        return undefined as T;
    }
  },
  async listen(event: string, cb: Cb) {
    listeners[event].push(cb);
    return () => {
      listeners[event] = listeners[event].filter((x) => x !== cb);
    };
  },
};
