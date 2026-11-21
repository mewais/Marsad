import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./theme/tokens.css";
import "./theme/base.css";
import "./ui/ui.css";
import "./features/session/session.css";
import "./features/inbox/inbox.css";
import { loadTheme, applyTheme } from "./lib/theme";
import { applySurface, loadSurface } from "./lib/surface";

// This device's last theme and window background, before the first paint (no flash); the
// hub's settings replace them once they arrive.
applyTheme(loadTheme());
{
  const s = loadSurface();
  applySurface(s.surface, s.strength);
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
