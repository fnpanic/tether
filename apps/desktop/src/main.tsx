import React, { Suspense, useEffect } from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { applyTheme } from "./lib/theme";
import { Home } from "./pages/Home";
import { Session } from "./pages/Session";
import { ConnectionManager } from "./pages/ConnectionManager";
import { inTauri, invoke } from "./lib/ipc";
import { useMode } from "./lib/mode";
import { useToast } from "./components/ui";
import { errorText } from "./lib/policy";
import { AgentConsentDialog } from "./components/AgentHere";

// Advanced mode is its own chunk: the simple home view never downloads or parses it.
const Hosts = React.lazy(() => import("./pages/Hosts").then((m) => ({ default: m.Hosts })));
const lazyHosts = (
  <Suspense fallback={<div className="h-full bg-canvas" />}>
    <Hosts />
  </Suspense>
);

/** The main window's home view: Simple (Home) or Advanced (Hosts), docs/advanced.md. */
function Main() {
  const { mode } = useMode();
  useEffect(() => {
    invoke("main_window_fit", { advanced: mode === "advanced" }).catch(() => {});
  }, [mode]);
  // Messages from outside the page (a refused `--connect` to this device's own ID): app.rs notify.
  const toast = useToast();
  useEffect(() => {
    const take = () => invoke<string | null>("take_notice").then((m) => m && toast.show(errorText(m)), () => {});
    if (!inTauri) return;
    let off: (() => void) | undefined;
    let dead = false;
    import("@tauri-apps/api/event")
      .then(({ listen }) => listen("notice", take))
      .then((u) => (dead ? u() : (off = u)))
      .then(take);
    return () => {
      dead = true;
      off?.();
    };
  }, []);
  return (
    <>
      {mode === "advanced" ? lazyHosts : <Home />}
      {/* The health agent's pre-dialog, in either view (agent_elevated.rs). */}
      <AgentConsentDialog onNotice={toast.show} />
      {toast.node}
    </>
  );
}

applyTheme();

function route() {
  const hash = window.location.hash.replace(/^#/, "") || "/";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  if (path.startsWith("/session/")) {
    return <Session sessionKey={path.slice("/session/".length)} peer={params.get("peer") ?? ""} conn={params.get("conn") ?? "default"} viewOnly={params.get("view_only") === "1"} />;
  }
  if (path === "/hosts") return lazyHosts;
  if (path === "/cm") return <ConnectionManager />;
  return <Main />;
}

/** A page that throws shows what failed instead of an empty window. */
class Crash extends React.Component<{ children: React.ReactNode }, { error: string | null }> {
  state = { error: null as string | null };
  static getDerivedStateFromError(e: unknown) {
    return { error: String((e as Error)?.stack ?? e) };
  }
  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" className="flex h-full flex-col items-center justify-center gap-3 bg-canvas p-6 text-center text-ink">
        <div className="text-callout font-semibold">Something went wrong in this window</div>
        <pre className="max-h-48 max-w-full overflow-auto whitespace-pre-wrap text-left font-mono text-footnote text-muted">{this.state.error}</pre>
        <button className="rounded-full bg-accent px-4 py-1.5 text-caption text-on-accent" onClick={() => location.reload()}>
          Reload
        </button>
      </div>
    );
  }
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Crash>{route()}</Crash>
  </React.StrictMode>,
);

// Windows start hidden to avoid a white flash; reveal after the first paint.
// macOS (WKWebView) runs no animation frames while the window is hidden, so a timer reveals it too.
/** A window built larger than its screen (1366x768 at 150 % is 911x512) and centred starts with
 *  its title bar above the screen: shrink it to the work area and centre it again. */
async function fitScreen(w: import("@tauri-apps/api/window").Window) {
  // A fullscreen session window (--fullscreen) or a maximized one already fits its screen.
  if ((await w.isFullscreen()) || (await w.isMaximized())) return;
  const { LogicalSize } = await import("@tauri-apps/api/dpi");
  const s = (await w.innerSize()).toLogical(await w.scaleFactor());
  const fw = Math.min(s.width, screen.availWidth), fh = Math.min(s.height, screen.availHeight);
  if (fw >= s.width && fh >= s.height) return;
  await w.setMinSize(new LogicalSize(Math.min(640, fw), Math.min(400, fh)));
  await w.setSize(new LogicalSize(fw, fh));
  await w.center();
}

if (inTauri) {
  let shown = false;
  const reveal = async () => {
    if (shown) return;
    shown = true;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const w = getCurrentWindow();
    // The CM window may already be minimized by a session that started before the first paint.
    if (await w.isMinimized()) return;
    await fitScreen(w).catch(() => {});
    await w.show();
    await w.setFocus();
  };
  requestAnimationFrame(() => requestAnimationFrame(reveal));
  setTimeout(reveal, 150);
}
