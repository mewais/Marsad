import { useEffect, useState } from "react";
import { registerAction } from "../commands/registry";
import { useNavigate } from "../router";
import { useShortcut } from "../shortcuts/useShortcut";
import { useActions } from "../state";
import { ToastRegion, toast } from "../ui";
import { applyTheme, loadTheme } from "../lib/theme";
import { CommandPalette } from "./CommandPalette";
import { Rail } from "./Rail";
import { ShortcutHelp } from "./ShortcutHelp";
import { StatusBar } from "./StatusBar";
import { TitleBar } from "./TitleBar";
import "./shell.css";

export function AppShell({ children }: { children: React.ReactNode }) {
  const nav = useNavigate();
  const actions = useActions();
  const [palette, setPalette] = useState(false);
  const [help, setHelp] = useState(false);

  useShortcut({ id: "palette", keys: ["mod+k"], title: "Command palette", group: "Global", run: () => setPalette(true), allowInInput: true });
  useShortcut({ id: "help", keys: ["?"], title: "Show shortcuts", group: "Global", run: () => setHelp(true) });
  useShortcut({ id: "new", keys: ["n"], title: "New session", group: "Global", run: () => toast({ tone: "info", title: "New session is not built yet" }) });
  useShortcut({ id: "go-inbox", keys: ["g", "i"], title: "Go to Inbox", group: "Global", run: () => nav("/inbox") });
  useShortcut({ id: "go-ws", keys: ["g", "w"], title: "Go to Workstreams", group: "Global", run: () => nav("/workstreams") });
  useShortcut({ id: "go-hosts", keys: ["g", "h"], title: "Go to Hosts", group: "Global", run: () => nav("/hosts") });

  useEffect(() => {
    applyTheme(loadTheme());
    const offs = [
      registerAction({ id: "go-inbox", title: "Go to Inbox", section: "Navigate", icon: "inbox", shortcut: "G I", run: () => nav("/inbox") }),
      registerAction({ id: "go-ws", title: "Go to Workstreams", section: "Navigate", icon: "graph", shortcut: "G W", run: () => nav("/workstreams") }),
      registerAction({ id: "go-hosts", title: "Go to Hosts", section: "Navigate", icon: "server", shortcut: "G H", run: () => nav("/hosts") }),
      registerAction({ id: "go-config", title: "Go to Config", section: "Navigate", icon: "layers", run: () => nav("/config") }),
      registerAction({ id: "go-settings", title: "Go to Settings", section: "Navigate", icon: "gear", run: () => nav("/settings") }),
      registerAction({ id: "mark-all-seen", title: "Mark all finished sessions as seen", section: "Sessions", icon: "eye", run: () => actions.markAllSeen() }),
      registerAction({ id: "help", title: "Show keyboard shortcuts", section: "Help", icon: "slash", shortcut: "?", run: () => setHelp(true) }),
    ];
    return () => offs.forEach((o) => o());
  }, [nav, actions]);

  return (
    <div className="app">
      <TitleBar onPalette={() => setPalette(true)} />
      <div className="body">
        <Rail />
        <main className="main" id="main">
          {children}
        </main>
      </div>
      <StatusBar onHelp={() => setHelp(true)} />
      <CommandPalette open={palette} onClose={() => setPalette(false)} />
      <ShortcutHelp open={help} onClose={() => setHelp(false)} />
      <ToastRegion />
    </div>
  );
}
