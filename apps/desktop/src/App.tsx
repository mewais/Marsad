import { lazy, Suspense, useEffect, useMemo } from "react";
import { createBackend, type Backend } from "./bridge";
import { Placeholder } from "./features/Placeholder";
import { InboxPage } from "./features/inbox/InboxPage";
import { SessionPage } from "./features/session/SessionPage";
import { HostsPage } from "./features/hosts/HostsPage";
import { ConfigPage } from "./features/config/ConfigPage";
import { SettingsPage } from "./features/settings/SettingsPage";
import { WorkstreamsPage } from "./features/workstreams/WorkstreamsPage";

// The graph (and ELK) load only when a workstream opens.
const GraphPage = lazy(() => import("./features/workstreams/GraphPage").then((m) => ({ default: m.GraphPage })));
import { hashHistory, RouterProvider, sessionPath, useNavigate, useRoute, type History } from "./router";
import { AppShell } from "./shell/AppShell";
import { installShortcuts } from "./shortcuts/registry";
import { AppProvider, createApp } from "./state";
import { IconSprite } from "./ui";

/** The desktop shell's `navigate` event (a notification brought the app forward). */
function ShellNavigate({ backend }: { backend: Backend }) {
  const nav = useNavigate();
  useEffect(() => backend.onNavigate?.((t) => nav(sessionPath(t.host, t.session))), [backend, nav]);
  return null;
}

function Routes() {
  const r = useRoute();
  switch (r.name) {
    case "inbox":
      return <InboxPage />;
    case "session":
      return <SessionPage key={`${r.host}/${r.id}`} host={r.host} id={r.id} tab={r.tab} />;
    case "workstreams":
      return <WorkstreamsPage />;
    case "workstream":
      return <Suspense fallback={<div className="placeholder faint">Loading the graph…</div>}><GraphPage key={r.id} id={r.id} /></Suspense>;
    case "hosts":
      return <HostsPage q={r.q} />;
    case "config":
      return <ConfigPage q={r.q} />;
    case "settings":
      return <SettingsPage q={r.q} />;
    default:
      return <Placeholder title="Not found" icon="warn" />;
  }
}

export function App({ backend, history }: { backend?: Backend; history?: History }) {
  const app = useMemo(() => createApp(backend ?? createBackend()), [backend]);
  const hist = useMemo(() => history ?? hashHistory(), [history]);
  useEffect(() => installShortcuts(), []);
  useEffect(() => app.dispose, [app]);
  return (
    <AppProvider value={app}>
      <RouterProvider history={hist}>
        <IconSprite />
        <ShellNavigate backend={app.backend} />
        <AppShell>
          <Routes />
        </AppShell>
      </RouterProvider>
    </AppProvider>
  );
}
