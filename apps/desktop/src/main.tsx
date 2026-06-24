import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { applyUiScale, getUiScale } from "./theme/uiScale";

// Apply the saved per-machine UI scale before first paint.
applyUiScale(getUiScale());

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
