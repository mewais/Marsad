import { useEffect, useMemo, useState } from "react";
import type { DriverInventory, EffectiveConfig, RecentProject } from "../../bridge/wire";
import { agoLong } from "../../lib/format";
import { useNavigate, sessionPath } from "../../router";
import { useShortcut } from "../../shortcuts/useShortcut";
import { linkView, useActions, useApi, useAppState, useHosts } from "../../state";
import { Button, Dialog, Icon, Kbd, SearchField, Segmented, Select, toast } from "../../ui";
import type { NewSessionPreset } from "../../shell/dialogs";
import { defaultsFor, launchRequest, mostRecent, resumeRows, validate, type NewSessionForm } from "./model";
import { DriverPick, HostPick, ResumeList } from "./parts";
import { ProjectField } from "./ProjectField";
import { useInventory } from "./useInventory";
import "./newsession.css";

export interface NewSessionDialogProps {
  open: boolean;
  preset?: NewSessionPreset | null;
  onClose: () => void;
  /** Design mode ("Add agent"): return the form instead of launching. */
  onAdd?: (form: NewSessionForm, inv: DriverInventory) => void;
  title?: string;
  subtitle?: string;
  /** Only "New conversation" (a workstream node launch spec cannot resume). */
  newOnly?: boolean;
}

const blank = (host: string): NewSessionForm => ({ host, driver: "", cwd: "", model: "", mode: "", start: "new", resumeId: "", latest: false, prompt: "" });

export function NewSessionDialog({ open, preset, onClose, onAdd, newOnly, title = "New session", subtitle = "Starts on the host you pick and appears in your inbox" }: NewSessionDialogProps) {
  const hosts = useHosts();
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const settings = useAppState((s) => s.settings);
  const allSessions = useAppState((s) => s.sessions);
  const firstUp = hosts.find((h) => linkView(h.link, h.authPrompt).connected)?.id ?? "";
  const [f, setF] = useState<NewSessionForm>(() => blank(firstUp));
  const [recent, setRecent] = useState<RecentProject[]>([]);
  const [q, setQ] = useState("");
  const [eff, setEff] = useState<EffectiveConfig | null>(null);
  const set = (p: Partial<NewSessionForm>) => setF((x) => ({ ...x, ...p }));

  useEffect(() => {
    if (!open) return;
    const host = preset?.host ?? (typeof settings["defaults.host"] === "string" ? (settings["defaults.host"] as string) : firstUp);
    setF({ ...blank(host), driver: preset?.driver ?? "", cwd: preset?.cwd ?? "" });
    setQ("");
    api.store.recent().then((r) => {
      setRecent(r.projects);
      if (!preset?.cwd) setF((x) => (x.cwd ? x : { ...x, cwd: r.projects.find((p) => p.host === x.host)?.path ?? "" }));
    }, () => setRecent([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const host = hosts.find((h) => h.id === f.host);
  const hostUp = !!host && linkView(host.link, host.authPrompt).connected;
  const { drivers, loading, error } = useInventory(f.host, f.cwd, open && hostUp);
  const inv = drivers.find((d) => d.driver === f.driver);

  // Pick a driver (settings default, else the first installed) and the CLI's own defaults —
  // unless one is chosen and not known to be missing: a folder's inventory starts out
  // `refreshing` (not yet resolved), which must not override the user's pick.
  useEffect(() => {
    if (!open || !drivers.length) return;
    if (f.driver && (!inv || inv.installed || inv.refreshing)) return;
    const pref = typeof settings["defaults.driver"] === "string" ? (settings["defaults.driver"] as string) : "";
    const d = drivers.find((x) => x.driver === pref && x.installed) ?? drivers.find((x) => x.installed);
    if (d) set({ driver: d.driver, ...defaultsFor(d), resumeId: "" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, drivers, inv, settings]);

  const hostSessions = useMemo(() => Object.entries(allSessions).filter(([k]) => k.startsWith(f.host + "/")).map(([, s]) => s), [allSessions, f.host]);
  const rows = useMemo(() => resumeRows(inv, hostSessions, q), [inv, hostSessions, q]);
  const latest = mostRecent(useMemo(() => resumeRows(inv, hostSessions), [inv, hostSessions]));

  // Effective-config summary for this launch (preview; hub computes it).
  useEffect(() => {
    if (!open || !inv?.installed) return setEff(null);
    let off = false;
    // Asked once typing in the folder field settles: every folder asked about is discovered.
    const h = setTimeout(() => {
      api.config.effective({ host: f.host, driver: f.driver, project: f.cwd || null }).then((e) => !off && setEff(e), () => !off && setEff(null));
    }, 500);
    return () => {
      off = true;
      clearTimeout(h);
    };
  }, [api, open, f.host, f.driver, f.cwd, inv?.installed]);

  const problem = validate(f, inv, hostUp);
  const pk = "launch";
  const submit = async () => {
    if (problem || !inv) return;
    if (onAdd) {
      onAdd(f, inv);
      onClose();
      return;
    }
    const r = await actions.run(pk, "Could not start the session", () => api.launch(launchRequest(f)));
    if (!r) return;
    onClose();
    toast({ tone: "ok", title: f.start === "continue" ? "Session continued" : "Session started", detail: `${inv.display_name} on ${host?.name ?? f.host}` });
    for (const u of r.unsupported) toast({ tone: "warn", title: "Started without one setting", detail: `${u.item}: ${u.reason}` });
    nav(sessionPath(r.host, r.id));
  };
  useShortcut({ id: "ns-submit", keys: ["mod+enter"], title: "Start the session", group: "New session", allowInInput: true, priority: 20, when: () => open, run: () => void submit() });
  const pending = useAppState((s) => !!s.pending[pk]);

  const changed = eff?.items.filter((i) => i.state !== "inherited").length ?? 0;
  const auto = eff?.items.filter((i) => i.state === "automatic").length ?? 0;
  const verb = onAdd ? "Add to design" : f.start === "continue" ? "Continue session" : "Start session";

  return (
    <Dialog open={open} onClose={onClose} title={title} subtitle={subtitle} width={960} className="nsdlg"
      footer={
        <>
          <span className="faint ns-problem" role="status">{problem || (onAdd ? "The Wasl connector will be added automatically." : "")}</span>
          <span className="sp" />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={onAdd ? "plus" : f.start === "continue" ? "refresh" : "send"} disabled={!!problem} pending={pending} onClick={() => void submit()}>
            {verb} {onAdd ? null : <Kbd>Ctrl Enter</Kbd>}
          </Button>
        </>
      }>
      <div className="ns">
        <div className="col">
          <div className="field"><span className="lbl">Agent</span><DriverPick drivers={drivers} value={f.driver} onChange={(d) => set({ driver: d, ...defaultsFor(drivers.find((x) => x.driver === d)), resumeId: "" })} /></div>
          <div className="field"><span className="lbl">Host</span><HostPick hosts={hosts} value={f.host} onChange={(h) => set({ host: h, driver: "", resumeId: "" })} /></div>
          <ProjectField host={f.host} value={f.cwd} onChange={(cwd) => set({ cwd, resumeId: "" })} recent={recent} />
          <div className="two">
            <div className="field"><span className="lbl">Model</span>
              <Select label="Model" value={f.model || undefined} placeholder={inv?.models.length ? "Select…" : "Reported after start"} options={(inv?.models ?? []).map((m) => ({ value: m.id, label: m.display_name || m.id, description: m.description }))} onChange={(model) => set({ model })} />
            </div>
            <div className="field"><span className="lbl">Mode</span>
              <Select label="Mode" value={f.mode || undefined} placeholder={inv?.modes.length ? "Select…" : "Reported after start"} options={(inv?.modes ?? []).map((m) => ({ value: m.id, label: m.label, description: m.description }))} onChange={(mode) => set({ mode })} />
            </div>
          </div>
          <span className="disc">
            <Icon name="refresh" size="xs" />
            {error ? <span className="c-failed">{error}</span> : inv ? `Models and modes discovered from ${inv.binary_path?.split(/[\\/]/).pop() ?? inv.driver} ${inv.version ?? ""} on ${host?.name ?? f.host} · ${inv.models.length} models, ${inv.modes.length} modes${loading ? " · refreshing…" : ""}` : loading ? "Asking the host what is installed…" : ""}
          </span>
          {inv?.installed ? (
            <div className="profile">
              <Icon name="layers" size="sm" />
              <span className="ell">Config <b>{inv.display_name}'s own setup on {host?.name}</b>{changed - auto > 0 ? ` + ${changed - auto} Marsad override${changed - auto > 1 ? "s" : ""}` : ""}{auto ? " + Wasl" : ""}</span>
              <span className="sp" />
              <button type="button" className="linkbtn" onClick={() => { onClose(); nav(`/config?driver=${encodeURIComponent(f.driver)}&host=${encodeURIComponent(f.host)}`); }}>View</button>
            </div>
          ) : null}
        </div>
        <div className="col">
          {newOnly ? null : <div className="field"><span className="lbl">Start</span>
            <Segmented label="Start" value={f.start} onChange={(start) => set({ start, latest: start === "continue" && !!inv?.capabilities.continue_latest, resumeId: start === "continue" && !inv?.capabilities.continue_latest ? latest?.id ?? "" : "" })}
              options={[{ value: "new", label: "New conversation" }, { value: "continue", label: "Continue an existing one" }]} />
          </div>}
          {f.start === "continue" ? (
            <>
              {inv?.capabilities.continue_latest ? (
                <div className="recent">
                  <Icon name="clock" size="sm" />
                  <div className="tx">
                    <div><b>Continue most recent</b> <span className="faint">· {inv.display_name}'s own continue</span></div>
                    <div className="faint">{latest ? <>{latest.title || "(no title)"} · {agoLong(latest.modified_ms)}</> : `whatever ${inv.display_name} last ran in this folder`}</div>
                  </div>
                  <Button size="sm" onClick={() => set({ latest: true, resumeId: "" })} aria-pressed={f.latest}>{f.latest ? "Selected" : "Continue"}</Button>
                </div>
              ) : null}
              <div className="field">
                <SearchField placeholder="Search title or first prompt" aria-label="Search sessions" value={q} onChange={(e) => setQ(e.target.value)} />
                <span className="hint">{inv ? `${inv.sessions.length} ${inv.display_name} session${inv.sessions.length === 1 ? "" : "s"} in ${f.cwd || "this folder"} on ${host?.name}, including ones started in a terminal` : ""}</span>
                {loading && !inv?.sessions.length ? <div className="faint">Looking for sessions…</div> : <ResumeList rows={rows} value={f.latest ? "" : f.resumeId} onChange={(resumeId) => set({ resumeId, latest: false })} />}
              </div>
            </>
          ) : null}
          <div className="field">
            <label htmlFor="ns-prompt">{f.start === "continue" ? "Message to send on resume" : "First message"} <span className="faint optl">optional</span></label>
            <textarea id="ns-prompt" className="ta" rows={f.start === "continue" ? 2 : 6} value={f.prompt} onChange={(e) => set({ prompt: e.target.value })}
              placeholder={f.start === "continue" ? "Pick up where you left off…" : "What should the agent do?"} />
          </div>
        </div>
      </div>
    </Dialog>
  );
}
