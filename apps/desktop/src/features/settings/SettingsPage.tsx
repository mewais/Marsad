import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { DaemonInfo, JsonValue, NotificationPrefs } from "../../bridge/wire";
import { applyTheme, loadTheme, type ThemeChoice } from "../../lib/theme";
import { useStore } from "../../lib/store";
import { useNavigate } from "../../router";
import { allShortcuts } from "../../shortcuts/catalog";
import { prettyKey, shortcutStore } from "../../shortcuts/registry";
import { linkView, useActions, useApi, useApp, useAppState, useHosts } from "../../state";
import { Card, Icon, Kbd, Segmented, Select, type IconName } from "../../ui";
import "./settings.css";

const SECTIONS: { id: string; label: string; icon: IconName }[] = [
  { id: "general", label: "General", icon: "gear" },
  { id: "notifications", label: "Notifications", icon: "bell" },
  { id: "keyboard", label: "Keyboard", icon: "slash" },
  { id: "about", label: "About", icon: "tower" },
];

function Toggle({ label, hint, on, onChange, disabled }: { label: string; hint?: string; on: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <div className="setrow">
      <div className="setrow-t"><div className="h3">{label}</div>{hint ? <div className="faint small">{hint}</div> : null}</div>
      <button type="button" role="switch" className="tg" aria-checked={on} aria-label={label} disabled={disabled} onClick={() => onChange(!on)} />
    </div>
  );
}

/** One setting stored in the hub (`store.settings.*`), applied optimistically. */
function useSetting<T extends JsonValue>(key: string, fallback: T): [T, (v: T) => void] {
  const { store } = useApp();
  const api = useApi();
  const actions = useActions();
  const v = useAppState((s) => (key in s.settings ? (s.settings[key] as T) : fallback));
  const set = (next: T) => {
    store.set((s) => ({ ...s, settings: { ...s.settings, [key]: next } }));
    void actions.run(`setting:${key}`, "Could not save the setting", () => api.store.setSetting(key, next));
  };
  return [v, set];
}

function General() {
  const [theme, setThemeSetting] = useSetting<string>("theme", loadTheme());
  const [driver, setDriver] = useSetting<string>("defaults.driver", "");
  const [host, setHost] = useSetting<string>("defaults.host", "");
  const drivers = useAppState((s) => s.drivers);
  const order = useAppState((s) => s.driverOrder);
  const hosts = useHosts();
  const setTheme = (t: ThemeChoice) => {
    applyTheme(t);
    setThemeSetting(t);
  };
  return (
    <>
      <div><h1 className="h1">General</h1><div className="muted">How Marsad looks and what a new session starts with.</div></div>
      <Card className="setcard">
        <div className="setrow"><div className="setrow-t"><div className="h3">Theme</div><div className="faint small">System follows your OS setting.</div></div>
          <Segmented<ThemeChoice> label="Theme" value={(theme as ThemeChoice) || "system"} onChange={setTheme} options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} /></div>
        <div className="setrow"><div className="setrow-t"><div className="h3">Default agent</div><div className="faint small">Preselected in New session when it is installed on the host.</div></div>
          <Select label="Default agent" value={driver || "-"} options={[{ value: "-", label: "First installed" }, ...order.map((d) => ({ value: d, label: drivers[d]?.displayName ?? d }))]} onChange={(v) => setDriver(v === "-" ? "" : v)} /></div>
        <div className="setrow"><div className="setrow-t"><div className="h3">Default host</div><div className="faint small">Preselected in New session when it is connected.</div></div>
          <Select label="Default host" value={host || "-"} options={[{ value: "-", label: "First connected" }, ...hosts.map((h) => ({ value: h.id, label: h.name, description: linkView(h.link, h.authPrompt).word }))]} onChange={(v) => setHost(v === "-" ? "" : v)} /></div>
      </Card>
    </>
  );
}

const EVENTS: { key: keyof NotificationPrefs; label: string; icon: IconName; cls: string }[] = [
  { key: "needs_you", label: "Needs you: a permission, question, plan or handoff", icon: "needs", cls: "needs" },
  { key: "failed", label: "A session failed", icon: "failed", cls: "failed" },
  { key: "finished", label: "A session finished", icon: "done", cls: "done" },
  { key: "host_lost", label: "A host lost its connection", icon: "refresh", cls: "needs" },
];

function Notifications() {
  const prefs = useAppState((s) => s.notifications);
  const { store } = useApp();
  const api = useApi();
  const actions = useActions();
  const [sound, setSound] = useSetting<boolean>("notify.sound", true);
  const [dnd, setDnd] = useSetting<boolean>("notify.dnd", false);
  const [unfocused, setUnfocused] = useSetting<boolean>("notify.only_unfocused", true);
  const set = (k: keyof NotificationPrefs, v: boolean) => {
    if (!prefs) return;
    const next = { ...prefs, [k]: v };
    store.set((s) => ({ ...s, notifications: next }));
    void actions.run(`notify:${k}`, "Could not save notification settings", async () => {
      const saved = await api.store.setNotifications(next);
      store.set((s) => ({ ...s, notifications: saved }));
      return saved;
    });
  };
  return (
    <>
      <div><h1 className="h1">Notifications</h1><div className="muted">Choose which events reach you. Errors always show in the inbox, whatever you pick here.</div></div>
      <Card className="setcard">
        {!prefs ? <div className="faint small">Loading…</div> : EVENTS.map((e) => (
          <div key={e.key} className="setrow"><div className="setrow-t"><div className="h3"><span className={`c-${e.cls}`}><Icon name={e.icon} size="sm" /></span> {e.label}</div></div>
            <button type="button" role="switch" className="tg" aria-checked={prefs[e.key]} aria-label={e.label} onClick={() => set(e.key, !prefs[e.key])} /></div>
        ))}
      </Card>
      <Card className="setcard">
        <Toggle label="Play a sound" hint="With desktop notifications for “Needs you” and “Failed”." on={sound} onChange={setSound} />
        <Toggle label="Only when Marsad is not focused" hint="Desktop notifications stay quiet while you are looking at the app." on={unfocused} onChange={setUnfocused} />
        <Toggle label="Do not disturb" hint="No desktop notifications or sounds. The inbox and the tray badge still update." on={dnd} onChange={setDnd} />
      </Card>
    </>
  );
}

function Keyboard() {
  const list = allShortcuts(useStore(shortcutStore, (s) => s));
  const groups = new Map<string, typeof list>();
  for (const s of list) groups.set(s.group, [...(groups.get(s.group) ?? []), s]);
  return (
    <>
      <div><h1 className="h1">Keyboard</h1><div className="muted">Every shortcut in Marsad. Press <Kbd>?</Kbd> anywhere to see them.</div></div>
      {[...groups.entries()].map(([g, items]) => (
        <Card key={g} className="setcard">
          <div className="eyebrow">{g}</div>
          <table className="t"><tbody>{items.map((s) => <tr key={s.id}><td>{s.title}</td><td className="r">{s.keys.map((k, i) => <Kbd key={i}>{prettyKey(k)}</Kbd>)}</td></tr>)}</tbody></table>
        </Card>
      ))}
    </>
  );
}

function About() {
  const api = useApi();
  const [info, setInfo] = useState<DaemonInfo | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    api.daemonInfo().then(setInfo, (e) => setErr(errorMessage(e)));
  }, [api]);
  return (
    <>
      <div><h1 className="h1">About</h1><div className="muted">Marsad <span lang="ar">مرصد</span>: one place to watch and steer coding agents on all your machines.</div></div>
      <Card className="setcard">
        <dl className="kvl wide">
          <dt>App</dt><dd>Marsad desktop 0.1.0</dd>
          <dt>Daemon build</dt><dd className="mono">{info ? info.version.build : err || "…"}</dd>
          <dt>Target</dt><dd className="mono">{info?.version.target ?? ""}</dd>
          <dt>Hub</dt><dd>{info ? (info.hub ? "This machine" : "No") : ""}</dd>
          <dt>Data folder</dt><dd className="mono">{info?.data_dir ?? ""}</dd>
        </dl>
      </Card>
    </>
  );
}

/** Settings: General, Notifications, Keyboard, About. */
export function SettingsPage({ q }: { q: Record<string, string> }) {
  const nav = useNavigate();
  const sec = SECTIONS.some((s) => s.id === q.s) ? q.s : "general";
  return (
    <div className="set">
      <nav className="setnav" aria-label="Settings sections">
        <div className="eyebrow setnav-h">Settings</div>
        {SECTIONS.map((s) => (
          <a key={s.id} href={`#/settings?s=${s.id}`} className={`lr-item${s.id === sec ? " on" : ""}`} aria-current={s.id === sec ? "page" : undefined} onClick={(e) => { e.preventDefault(); nav(`/settings?s=${s.id}`); }}>
            <Icon name={s.icon} size="sm" />{s.label}
          </a>
        ))}
      </nav>
      <div className="setbody">{sec === "general" ? <General /> : sec === "notifications" ? <Notifications /> : sec === "keyboard" ? <Keyboard /> : <About />}</div>
    </div>
  );
}
