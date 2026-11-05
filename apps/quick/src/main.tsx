import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { App } from "./App";
import { inTauri } from "./ipc";

const mq = window.matchMedia("(prefers-color-scheme: dark)");
const theme = () => document.documentElement.classList.toggle("dark", mq.matches);
theme();
mq.addEventListener("change", theme);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// The window starts hidden to avoid a white flash; reveal after the first paint.
// macOS (WKWebView) runs no animation frames while the window is hidden, so a timer reveals it too.
if (inTauri) {
  let shown = false;
  const reveal = async () => {
    if (shown) return;
    shown = true;
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    // Already minimized by a session that started before the first paint.
    if (await getCurrentWindow().isMinimized()) return;
    await getCurrentWindow().show();
    await getCurrentWindow().setFocus();
  };
  requestAnimationFrame(() => requestAnimationFrame(reveal));
  setTimeout(reveal, 150);
}
