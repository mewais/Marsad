import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import "./theme/tokens.css";
import "./theme/base.css";
import "./ui/ui.css";
import "./features/session/session.css";
import "./features/inbox/inbox.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
