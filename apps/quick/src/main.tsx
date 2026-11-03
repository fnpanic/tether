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
if (inTauri) {
  requestAnimationFrame(() =>
    requestAnimationFrame(async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().show();
      await getCurrentWindow().setFocus();
    }),
  );
}
