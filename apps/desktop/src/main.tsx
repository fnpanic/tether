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

ReactDOM.createRoot(document.getElementById("root")!).render(<React.StrictMode>{route()}</React.StrictMode>);

// Windows start hidden to avoid a white flash; reveal after the first paint.
if (inTauri) {
  requestAnimationFrame(() =>
    requestAnimationFrame(async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const w = getCurrentWindow();
      await w.show();
      await w.setFocus();
    }),
  );
}
