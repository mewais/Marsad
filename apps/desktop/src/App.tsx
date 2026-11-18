import { useEffect, useMemo } from "react";
import { createBackend, type Backend } from "./bridge";
import { Placeholder } from "./features/Placeholder";
import { InboxPage } from "./features/inbox/InboxPage";
import { SessionPage } from "./features/session/SessionPage";
import { hashHistory, RouterProvider, useRoute, type History } from "./router";
import { AppShell } from "./shell/AppShell";
import { installShortcuts } from "./shortcuts/registry";
import { AppProvider, createApp } from "./state";
import { IconSprite } from "./ui";

function Routes() {
  const r = useRoute();
  switch (r.name) {
    case "inbox":
      return <InboxPage />;
    case "session":
      return <SessionPage key={`${r.host}/${r.id}`} host={r.host} id={r.id} tab={r.tab} />;
    case "workstreams":
      return <Placeholder title="Workstreams" icon="graph" />;
    case "hosts":
      return <Placeholder title="Hosts" icon="server" />;
    case "config":
      return <Placeholder title="Config" icon="layers" />;
    case "settings":
      return <Placeholder title="Settings" icon="gear" settings />;
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
        <AppShell>
          <Routes />
        </AppShell>
      </RouterProvider>
    </AppProvider>
  );
}
