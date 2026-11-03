import React from "react";
import ReactDOM from "react-dom/client";
import "./styles.css";
import { applyTheme } from "./lib/theme";
import { App } from "./App";

applyTheme();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
