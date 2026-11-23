import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { DaemonInfo, JsonValue, NotificationPrefs } from "../../bridge/wire";
import { applyTheme, loadTheme, type ThemeChoice } from "../../lib/theme";
import { applySurface, clampStrength, loadSurface, surfaceSupport, type SurfaceChoice } from "../../lib/surface";
import { useStore } from "../../lib/store";
import { useNavigate } from "../../router";
import { allShortcuts } from "../../shortcuts/catalog";
import { prettyKey, shortcutStore } from "../../shortcuts/registry";
import { DEVELOPER_MODE, linkView, useActions, useApi, useApp, useAppState, useHosts, usePending, useReach, type LoadKey } from "../../state";
import { Button, Card, Icon, Kbd, Segmented, Select, type IconName } from "../../ui";
import { ExportCard } from "../diagnostics/ExportCard";
import "./settings.css";

const SECTIONS: { id: string; label: string; icon: IconName }[] = [
  { id: "general", label: "General", icon: "gear" },
  { id: "notifications", label: "Notifications", icon: "bell" },
  { id: "keyboard", label: "Keyboard", icon: "slash" },
  { id: "diagnostics", label: "Diagnostics", icon: "list" },
  { id: "about", label: "About", icon: "tower" },
];

function Toggle({ label, hint, on, onChange, pending }: { label: string; hint?: string; on: boolean; onChange: (v: boolean) => void; pending?: boolean }) {
  return (
    <div className="setrow">
      <div className="setrow-t"><div className="h3">{label}</div>{hint ? <div className="faint small">{hint}</div> : null}</div>
      <button type="button" role="switch" className="tg" aria-checked={on} aria-label={label} aria-busy={pending || undefined} disabled={pending} onClick={() => onChange(!on)} />
    </div>
  );
}

/**
 * One setting stored in the hub (`store.settings.*`). What it shows is what the hub has: a change
 * is sent, the control is busy while it is in flight, and the hub's reply is what updates it. If
 * the hub refuses (or cannot be reached) the control stays as it was and the error is shown.
 * `set` resolves to whether the hub accepted it.
 */
function useSetting<T extends JsonValue>(key: string, fallback: T) {
  const { store } = useApp();
  const api = useApi();
  const actions = useActions();
  const value = useAppState((s) => (key in s.settings ? (s.settings[key] as T) : fallback));
  const pending = !!usePending(`setting:${key}`);
  const set = async (next: T): Promise<boolean> =>
    (await actions.run(`setting:${key}`, "Could not save the setting", async () => {
      const saved = await api.store.setSetting(key, next);
      store.set((s) => ({ ...s, settings: { ...saved.values } }));
      return true;
    })) !== undefined;
  return { value, set, pending };
}

const LOAD_WORDS: Record<LoadKey, string> = { settings: "your settings", notifications: "your notification choices", seen: "what you have seen" };

/**
 * Shows `children` only once every document in `keys` has arrived from the hub: until then they would be the
 * defaults, not what is stored. Loading is said as loading; a failure is shown with its reason and a retry.
 * While the app has no link to the hub the controls are off (a change could not be saved).
 */
function Loaded({ keys, children }: { keys: LoadKey[]; children: React.ReactNode }) {
  const loads = useAppState((s) => s.loads);
  const reach = useReach();
  const actions = useActions();
  const failed = keys.find((k) => loads[k].state === "error");
  if (failed) {
    const l = loads[failed];
    return (
      <Card className="setcard">
        <div className="setrow">
          <div className="setrow-t"><div className="h3">Could not load {LOAD_WORDS[failed]}</div><div className="faint small" role="alert">{l.state === "error" ? l.error : ""}</div></div>
          <Button size="sm" icon="refresh" onClick={() => actions.reload(failed)}>Try again</Button>
        </div>
      </Card>
    );
  }
  if (keys.some((k) => loads[k].state === "loading")) return <Card className="setcard"><div className="faint small" role="status"><Icon name="working" size="xs" spin /> Loading {keys.map((k) => LOAD_WORDS[k]).join(" and ")}…</div></Card>;
  return (
    <fieldset className="setfs" disabled={!!reach}>
      {reach ? <div className="faint small" role="status" data-testid="settings-blocked">Changes cannot be saved right now. {reach}</div> : null}
      {children}
    </fieldset>
  );
}

function General() {
  const { value: theme, set: setThemeSetting, pending: themePending } = useSetting<string>("theme", loadTheme());
  const { value: driver, set: setDriver, pending: driverPending } = useSetting<string>("defaults.driver", "");
  const { value: host, set: setHost, pending: hostPending } = useSetting<string>("defaults.host", "");
  const drivers = useAppState((s) => s.drivers);
  const order = useAppState((s) => s.driverOrder);
  const hosts = useHosts();
  // The theme is painted at once (it is only how this window looks); if the hub does not take it, it is put back.
  const setTheme = async (t: ThemeChoice) => {
    applyTheme(t);
    if (!(await setThemeSetting(t))) applyTheme((theme as ThemeChoice) || "system");
  };
  const local = loadSurface();
  const { value: surface, set: setSurface, pending: surfacePending } = useSetting<string>("surface", local.surface);
  const { value: strength, set: setStrength, pending: strengthPending } = useSetting<number>("surface.strength", local.strength);
  const support = useStore(surfaceSupport, (s) => s);
  const translucent = surface === "translucent";
  // The slider follows the pointer in a draft (and previews it); the hub gets the value when it is let go.
  const [draft, setDraft] = useState<number | null>(null);
  const commitStrength = async () => {
    if (draft === null) return;
    const n = draft;
    if (n !== clampStrength(strength) && !(await setStrength(n))) applySurface(translucent ? "translucent" : "solid", clampStrength(strength), support);
    setDraft(null);
  };
  const surfaceHint = translucent && support?.hint ? support.hint : !translucent
    ? "Translucent lets your desktop show through the window, in either theme."
    : support && !support.transparent
      ? "Your desktop cannot show transparent windows (no compositor), so Marsad stays solid."
      : support?.effect
        ? `Blurred backdrop (${support.effect === "vibrancy" ? "macOS vibrancy" : support.effect === "mica" ? "Mica" : support.effect === "acrylic" ? "Acrylic" : "blur"}). The strength sets how much of it shows.${support.effect === "acrylic" || support.effect === "mica" ? " Windows draws it solid while Marsad is not the active window." : ""}`
        : "Blurred where the system offers it (Windows 11, macOS). On Linux the window is see-through without blur, if your desktop has a compositor.";
  return (
    <>
      <div><h1 className="h1">General</h1><div className="muted">How Marsad looks and what a new session starts with.</div></div>
      <Loaded keys={["settings"]}>
      <Card className="setcard">
        <div className="setrow"><div className="setrow-t"><div className="h3">Theme</div><div className="faint small">System follows your OS setting.</div></div>
          <Segmented<ThemeChoice> label="Theme" value={(theme as ThemeChoice) || "system"} onChange={(t) => void setTheme(t)} options={[{ value: "system", label: "System" }, { value: "light", label: "Light" }, { value: "dark", label: "Dark" }]} /></div>
        <div className="setrow"><div className="setrow-t"><div className="h3">Window background</div><div className="faint small" data-testid="surface-hint">{surfaceHint}</div></div>
          <Segmented<SurfaceChoice> label="Window background" value={translucent ? "translucent" : "solid"} onChange={(v) => void setSurface(v)} options={[{ value: "solid", label: "Solid" }, { value: "translucent", label: "Translucent" }]} /></div>
        {translucent ? <div className="setrow"><div className="setrow-t"><div className="h3">Translucency</div><div className="faint small">How much shows through. Text keeps its contrast at every step.</div></div>
          <input type="range" className="range" min={0} max={100} step={5} value={draft ?? clampStrength(strength)} aria-label="Translucency" aria-busy={strengthPending || undefined}
            onChange={(e) => { const n = Number(e.target.value); setDraft(n); applySurface("translucent", n, support); }}
            onPointerUp={() => void commitStrength()} onKeyUp={() => void commitStrength()} onBlur={() => void commitStrength()} /></div> : null}
        <div className="setrow"><div className="setrow-t"><div className="h3">Default agent</div><div className="faint small">Preselected in New session when it is installed on the host.</div></div>
          <Select label="Default agent" blocked={driverPending ? "Saving…" : null} value={driver || "-"} options={[{ value: "-", label: "First installed" }, ...order.map((d) => ({ value: d, label: drivers[d]?.displayName ?? d }))]} onChange={(v) => void setDriver(v === "-" ? "" : v)} /></div>
        <div className="setrow"><div className="setrow-t"><div className="h3">Default host</div><div className="faint small">Preselected in New session when it is connected.</div></div>
          <Select label="Default host" blocked={hostPending ? "Saving…" : null} value={host || "-"} options={[{ value: "-", label: "First connected" }, ...hosts.map((h) => ({ value: h.id, label: h.name, description: linkView(h.link, h.authPrompt).word }))]} onChange={(v) => void setHost(v === "-" ? "" : v)} /></div>
        {themePending || surfacePending ? <div className="faint small" role="status"><Icon name="working" size="xs" spin /> Saving…</div> : null}
      </Card>
      </Loaded>
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
  // One key for all four: each write sends every choice, so two in flight would overwrite each other.
  const saving = !!usePending("notify");
  const { value: unfocused, set: setUnfocused, pending: unfocusedPending } = useSetting<boolean>("notify.only_unfocused", true);
  const { value: dnd, set: setDnd, pending: dndPending } = useSetting<boolean>("notify.dnd", false);
  const set = (k: keyof NotificationPrefs, v: boolean) => {
    if (!prefs) return;
    const next = { ...prefs, [k]: v };
    void actions.run("notify", "Could not save notification settings", async () => {
      const saved = await api.store.setNotifications(next);
      store.set((s) => ({ ...s, notifications: saved }));
      return saved;
    });
  };
  return (
    <>
      <div><h1 className="h1">Notifications</h1><div className="muted">Choose which events reach you. Errors always show in the inbox, whatever you pick here.</div></div>
      <Loaded keys={["notifications", "settings"]}>
      <Card className="setcard">
        {prefs ? EVENTS.map((e) => (
          <div key={e.key} className="setrow"><div className="setrow-t"><div className="h3"><span className={`c-${e.cls}`}><Icon name={e.icon} size="sm" /></span> {e.label}</div></div>
            <button type="button" role="switch" className="tg" aria-checked={prefs[e.key]} aria-label={e.label} aria-busy={saving || undefined} disabled={saving} onClick={() => set(e.key, !prefs[e.key])} /></div>
        )) : null}
      </Card>
      <Card className="setcard">
        <Toggle label="Only when Marsad is not focused" hint="Desktop notifications stay quiet while you are looking at the app." on={unfocused} pending={unfocusedPending} onChange={(v) => void setUnfocused(v)} />
        <Toggle label="Do not disturb" hint="No desktop notifications. The inbox and the tray badge still update." on={dnd} pending={dndPending} onChange={(v) => void setDnd(v)} />
      </Card>
      </Loaded>
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

function Diagnostics() {
  const actions = useActions();
  const { value: developer, set, pending } = useSetting<boolean>(DEVELOPER_MODE, false);
  // The app records that the switch was turned only once the hub has taken it.
  const change = async (on: boolean) => {
    if (await set(on)) actions.record({ ev: "ui.developer_mode", on });
  };
  return (
    <>
      <div><h1 className="h1">Diagnostics</h1><div className="muted">What Marsad records about itself, and how to look at it.</div></div>
      {/* Outside `Loaded`: it needs the hub, not the settings, and says why it cannot export when the hub is unreachable. */}
      <ExportCard />
      <Loaded keys={["settings"]}>
        <Card className="setcard">
          <Toggle label="Developer mode" hint="Shows a Diagnostics tab on every session and a connection log on every host, with where each prompt went, how a session's state changed and why a host would not connect, as ids, sizes, timings and reasons and never your prompts, replies or files." on={developer} pending={pending} onChange={(v) => void change(v)} />
        </Card>
      </Loaded>
    </>
  );
}

function About() {
  const api = useApi();
  const hub = useAppState((s) => s.hub.state);
  const [version, setVersion] = useState<string | null>(null);
  const [versionErr, setVersionErr] = useState("");
  const [info, setInfo] = useState<DaemonInfo | null>(null);
  const [err, setErr] = useState("");
  useEffect(() => {
    api.appVersion().then(setVersion, (e) => setVersionErr(errorMessage(e)));
  }, [api]);
  // The daemon's build is asked of the daemon, whenever the app is connected to it; without a link there is nothing to show.
  useEffect(() => {
    setInfo(null);
    setErr("");
    if (hub !== "connected") return;
    let live = true;
    api.daemonInfo().then((i) => live && setInfo(i), (e) => live && setErr(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [api, hub]);
  const waiting = hub !== "connected" ? "Not connected to the local service" : err || "…";
  return (
    <>
      <div><h1 className="h1">About</h1><div className="muted">Marsad <span lang="ar">مرصد</span>: one place to watch and steer coding agents on all your machines.</div></div>
      <Card className="setcard">
        <dl className="kvl wide">
          <dt>App</dt><dd data-testid="app-version">Marsad desktop {version ?? (versionErr || "…")}</dd>
          <dt>Daemon build</dt><dd className="mono">{info ? info.version.build : waiting}</dd>
          <dt>Target</dt><dd className="mono">{info?.version.target ?? ""}</dd>
          <dt>Hub</dt><dd>{info ? (info.hub ? "This machine" : "No") : ""}</dd>
          <dt>Data folder</dt><dd className="mono">{info?.data_dir ?? ""}</dd>
        </dl>
      </Card>
    </>
  );
}

/** Settings: General, Notifications, Keyboard, Diagnostics, About. */
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
      <div className="setbody">{sec === "general" ? <General /> : sec === "notifications" ? <Notifications /> : sec === "keyboard" ? <Keyboard /> : sec === "diagnostics" ? <Diagnostics /> : <About />}</div>
    </div>
  );
}
