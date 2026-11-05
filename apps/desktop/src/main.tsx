import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { applyTheme } from "./lib/theme";
import { Home } from "./pages/Home";
import { Session } from "./pages/Session";
import { ConnectionManager } from "./pages/ConnectionManager";
import { inTauri } from "./lib/ipc";

applyTheme();

function route() {
  const hash = window.location.hash.replace(/^#/, "") || "/";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  if (path.startsWith("/session/")) {
    return <Session sessionKey={path.slice("/session/".length)} peer={params.get("peer") ?? ""} conn={params.get("conn") ?? "default"} />;
  }
  if (path === "/cm") return <ConnectionManager />;
  return <Home />;
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
        <button className="rounded-full bg-accent px-4 py-1.5 text-caption text-white" onClick={() => location.reload()}>
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
if (inTauri) {
  let shown = false;
  const reveal = async () => {
    if (shown) return;
    shown = true;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const w = getCurrentWindow();
    // The CM window may already be minimized by a session that started before the first paint.
    if (await w.isMinimized()) return;
    await w.show();
    await w.setFocus();
  };
  requestAnimationFrame(() => requestAnimationFrame(reveal));
  setTimeout(reveal, 150);
}
