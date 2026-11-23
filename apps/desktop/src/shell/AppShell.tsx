import { useEffect, useState } from "react";
import { registerAction } from "../commands/registry";
import { useNavigate } from "../router";
import { useShortcut } from "../shortcuts/useShortcut";
import { useActions, useApi, useAppState } from "../state";
import { ToastRegion } from "../ui";
import { openNewSession, openNewWorkstream } from "./dialogs";
import { GlobalDialogs } from "./GlobalDialogs";
import { PreviewHost } from "../features/media/PreviewHost";
import { applyTheme, loadTheme } from "../lib/theme";
import { applySurface, clampStrength, loadSurface, surfaceSupport } from "../lib/surface";
import { useStore } from "../lib/store";
import { CommandPalette } from "./CommandPalette";
import { ConnectionBanner } from "./ConnectionBanner";
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
  // The theme lives in the hub's settings; this device applies it as soon as it arrives.
  const theme = useAppState((s) => s.settings.theme);
  useEffect(() => {
    if (theme === "light" || theme === "dark" || theme === "system") applyTheme(theme);
  }, [theme]);
  // The window background, likewise: the native backdrop follows the choice, the CSS also the
  // strength and what the window turned out to support.
  const api = useApi();
  const surfaceSetting = useAppState((s) => s.settings.surface);
  const strengthSetting = useAppState((s) => s.settings["surface.strength"]);
  const local = loadSurface();
  const surface = surfaceSetting === "translucent" || surfaceSetting === "solid" ? surfaceSetting : local.surface;
  const strength = typeof strengthSetting === "number" ? clampStrength(strengthSetting) : local.strength;
  const support = useStore(surfaceSupport, (s) => s);
  useEffect(() => {
    let live = true;
    void actions.run("surface", "Could not change the window background", async () => {
      const sup = await api.windowSurface(surface === "translucent");
      // Only a translucent answer says anything about what the window can show.
      if (live) surfaceSupport.set(surface === "translucent" ? sup : null);
      return sup;
    }, { local: true });
    return () => {
      live = false;
    };
  }, [surface, api, actions]);
  useEffect(() => applySurface(surface, strength, support), [surface, strength, support]);

  useShortcut({ id: "palette", keys: ["mod+k"], title: "Command palette", group: "Global", run: () => setPalette(true), allowInInput: true });
  useShortcut({ id: "help", keys: ["?"], title: "Show shortcuts", group: "Global", run: () => setHelp(true) });
  useShortcut({ id: "new", keys: ["n"], title: "New session", group: "Global", run: () => openNewSession() });
  useShortcut({ id: "new-ws", keys: ["shift+n"], title: "New workstream", group: "Global", run: () => openNewWorkstream() });
  useShortcut({ id: "go-config", keys: ["g", "c"], title: "Go to Config", group: "Global", run: () => nav("/config") });
  useShortcut({ id: "go-settings", keys: ["g", "s"], title: "Go to Settings", group: "Global", run: () => nav("/settings") });
  useShortcut({ id: "go-inbox", keys: ["g", "i"], title: "Go to Inbox", group: "Global", run: () => nav("/inbox") });
  useShortcut({ id: "go-ws", keys: ["g", "w"], title: "Go to Workstreams", group: "Global", run: () => nav("/workstreams") });
  useShortcut({ id: "go-hosts", keys: ["g", "h"], title: "Go to Hosts", group: "Global", run: () => nav("/hosts") });

  useEffect(() => {
    applyTheme(loadTheme());
    const offs = [
      registerAction({ id: "go-inbox", title: "Go to Inbox", section: "Navigate", icon: "inbox", shortcut: "G I", run: () => nav("/inbox") }),
      registerAction({ id: "go-ws", title: "Go to Workstreams", section: "Navigate", icon: "graph", shortcut: "G W", run: () => nav("/workstreams") }),
      registerAction({ id: "go-hosts", title: "Go to Hosts", section: "Navigate", icon: "server", shortcut: "G H", run: () => nav("/hosts") }),
      registerAction({ id: "go-config", title: "Go to Config", section: "Navigate", icon: "layers", shortcut: "G C", run: () => nav("/config") }),
      registerAction({ id: "go-settings", title: "Go to Settings", section: "Navigate", icon: "gear", shortcut: "G S", run: () => nav("/settings") }),
      registerAction({ id: "new-session", title: "New session…", section: "Create", icon: "plus", shortcut: "N", keywords: "start launch continue resume agent", run: () => openNewSession() }),
      registerAction({ id: "new-workstream", title: "New workstream…", section: "Create", icon: "graph", shortcut: "⇧ N", keywords: "graph agents handoff", run: () => openNewWorkstream() }),
      registerAction({ id: "add-host", title: "Add host…", section: "Hosts", icon: "server", keywords: "ssh wsl machine", run: () => nav("/hosts?add=1") }),
      registerAction({ id: "settings-notifications", title: "Notification settings", section: "Settings", icon: "bell", run: () => nav("/settings?s=notifications") }),
      registerAction({ id: "settings-theme", title: "Appearance and theme", section: "Settings", icon: "eye", keywords: "dark light", run: () => nav("/settings?s=general") }),
      registerAction({ id: "settings-keys", title: "Keyboard shortcuts settings", section: "Settings", icon: "slash", run: () => nav("/settings?s=keyboard") }),
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
          <ConnectionBanner />
          {children}
        </main>
      </div>
      <StatusBar onHelp={() => setHelp(true)} />
      <CommandPalette open={palette} onClose={() => setPalette(false)} />
      <ShortcutHelp open={help} onClose={() => setHelp(false)} />
      <GlobalDialogs />
      <PreviewHost />
      <ToastRegion />
    </div>
  );
}
