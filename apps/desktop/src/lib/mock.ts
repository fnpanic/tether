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

function emit(event: string, name: string, payload: unknown) {
  for (const cb of listeners[event] ?? []) cb({ name, payload });
}

export const mock = {
  async invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    const a = args ?? {};
    switch (cmd) {
      case "status":
        return { id: "731204986", password: state.password, status: 1, key_confirmed: true, installed: false, version: "0.1.0" } as T;
      case "refresh_password":
        state.password = Math.random().toString(36).slice(2, 8);
        return undefined as T;
      case "recent_peers":
        return state.peers as T;
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
        setTimeout(() => emit("session", "peer_info", { hostname: "Design-Studio", username: "maria", platform: "Mac OS", displays: [{ x: 0, y: 0, width: 2560, height: 1440 }, { x: 2560, y: 0, width: 1920, height: 1080 }], current_display: 0 }), 600);
        setTimeout(() => emit("session", "connection_type", { secured: true, direct: true, stream_type: "" }), 650);
        setTimeout(() => emit("session", "quality", { fps: { 0: 60 }, delay: 14, target_bitrate: 3800, codec: "H264", speed: "4.2MB/s" }), 900);
        return undefined as T;
      case "cm_clients":
        return [{ id: 1, peer_id: "184392571", name: "maria@Design-Studio", authorized: false, keyboard: true, clipboard: true, audio: true, file: true, restart: true, recording: false, block_input: false, is_file_transfer: false }] as T;
      case "fs_home_dir":
        return "C:\\Users\\demo" as T;
      case "fs_local_dir":
        return { path: a.path, entries: [{ name: "Documents", type: 1, size: 0, modified: 1761900000 }, { name: "Downloads", type: 1, size: 0, modified: 1761900000 }, { name: "report-q3.pdf", type: 4, size: 2483021, modified: 1761900000 }, { name: "notes.txt", type: 4, size: 1204, modified: 1761900000 }] } as T;
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
