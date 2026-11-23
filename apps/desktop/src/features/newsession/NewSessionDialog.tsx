import { useEffect, useMemo, useRef, useState } from "react";
import type { DriverInventory, EffectiveConfig, PromptInput, RecentProject } from "../../bridge/wire";
import { errorMessage } from "../../bridge/api";
import { batchProblem, fromFile, fromPicked, revalidate, type PendingFile } from "../session/attachments";
import { PendingList } from "../session/Composer";
import { setDraft } from "../session/draft";
import { launchWithAttachments, type Step } from "./launch";
import { agoLong } from "../../lib/format";
import { inShell } from "../../lib/shell";
import { useNavigate, sessionPath } from "../../router";
import { useShortcut } from "../../shortcuts/useShortcut";
import { linkView, modelOptions, useActions, useApi, useAppState, useHosts, useReach } from "../../state";
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
  // This machine first, else the first connected host.
  const up = (h: (typeof hosts)[number]) => linkView(h.link, h.authPrompt).connected;
  const firstUp = (hosts.find((h) => h.local && up(h)) ?? hosts.find(up))?.id ?? "";
  const [f, setF] = useState<NewSessionForm>(() => blank(firstUp));
  const [recent, setRecent] = useState<RecentProject[]>([]);
  const [q, setQ] = useState("");
  const [eff, setEff] = useState<EffectiveConfig | null>(null);
  const set = (p: Partial<NewSessionForm>) => setF((x) => ({ ...x, ...p }));
  // Attachments of the first message: limits come from the chosen driver's descriptor on the chosen host.
  const [files, setFiles] = useState<PendingFile[]>([]);
  const [dropped, setDropped] = useState<string[]>([]);
  const [step, setStep] = useState<Step | null>(null);
  const [drag, setDrag] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const [infos, setInfos] = useState<Record<string, PromptInput | undefined>>({});
  useEffect(() => {
    if (!open || !f.host || !f.driver || onAdd) return;
    const k = `${f.host}/${f.driver}`;
    if (k in infos) return;
    let off = false;
    api.hostInfo(f.host).then(
      (i) => !off && setInfos((m) => ({ ...m, [k]: i.drivers.find((d) => d.id === f.driver)?.prompt_input })),
      () => !off && setInfos((m) => ({ ...m, [k]: undefined })),
    );
    return () => {
      off = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, f.host, f.driver, onAdd]);
  const promptInput: PromptInput | undefined = infos[`${f.host}/${f.driver}`];
  const canAttach = !onAdd && !!promptInput && promptInput.max_attachments > 0;
  // Switching driver or host re-validates what is attached; what the new agent cannot take is removed and said.
  useEffect(() => {
    if (!promptInput) return;
    setFiles((cur) => {
      const { keep, dropped: gone } = revalidate(cur, promptInput);
      if (!gone.length) return cur;
      cur.filter((x) => !keep.includes(x)).forEach((x) => x.thumb && URL.revokeObjectURL(x.thumb));
      setDropped(gone);
      return keep;
    });
  }, [promptInput]);
  const problemFiles = useMemo(() => (files.length && promptInput ? batchProblem(files, promptInput) : null), [files, promptInput]);
  const addFiles = (list: PendingFile[]) => {
    if (!list.length) return;
    setDropped([]);
    setFiles((fs) => [...fs, ...list]);
  };
  const removeFile = (k: string) =>
    setFiles((fs) => {
      fs.find((x) => x.key === k)?.thumb && URL.revokeObjectURL(fs.find((x) => x.key === k)!.thumb!);
      return fs.filter((x) => x.key !== k);
    });
  const attach = async () => {
    if (!inShell()) return fileInput.current?.click();
    const r = await actions.run("ns-pick", "Could not open the file picker", () => api.desktop.pickFiles("Attach files"), { local: true });
    if (r) addFiles(r.files.map((x) => fromPicked(api, x)));
  };

  useEffect(() => {
    if (!open) return;
    const host = preset?.host ?? (typeof settings["defaults.host"] === "string" ? (settings["defaults.host"] as string) : firstUp);
    setF({ ...blank(host), driver: preset?.driver ?? "", cwd: preset?.cwd ?? "" });
    setQ("");
    setFiles([]);
    setDropped([]);
    setStep(null);
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

  const problem = validate(f, inv, hostUp) || problemFiles || "";
  const reach = useReach(f.host);
  const pk = "launch";
  const submit = async () => {
    if (problem || reach || !inv) return;
    if (onAdd) {
      onAdd(f, inv);
      onClose();
      return;
    }
    if (files.length && canAttach) {
      const text = f.prompt.trim();
      const out = await launchWithAttachments(api, launchRequest(f), text, files, setStep);
      setStep(null);
      if (!out.ok && out.stage === "create") return toast({ tone: "err", title: "Could not start the session", detail: errorMessage(out.error) });
      onClose();
      const r = out.reply;
      if (!out.ok) {
        // The session exists: open it with the message and files waiting in its composer.
        setDraft(`${r.host}/${r.id}`, { text, files });
        toast({ tone: "err", title: out.stage === "upload" ? "The session started, but an attachment could not be uploaded" : "The session started, but the first message was not sent", detail: `${errorMessage(out.error)} Your message and attachments are in the composer.` });
      } else {
        files.forEach((x) => x.thumb && URL.revokeObjectURL(x.thumb));
        toast({ tone: "ok", title: "Session started", detail: `${inv.display_name} on ${host?.name ?? f.host}` });
      }
      for (const u of r.unsupported) toast({ tone: "warn", title: "Started without one setting", detail: `${u.item}: ${u.reason}` });
      return nav(sessionPath(r.host, r.id));
    }
    const r = await actions.run(pk, "Could not start the session", () => api.launch(launchRequest(f)));
    if (!r) return;
    onClose();
    toast({ tone: "ok", title: f.start === "continue" ? "Session continued" : "Session started", detail: `${inv.display_name} on ${host?.name ?? f.host}` });
    for (const u of r.unsupported) toast({ tone: "warn", title: "Started without one setting", detail: `${u.item}: ${u.reason}` });
    nav(sessionPath(r.host, r.id));
  };
  useShortcut({ id: "ns-submit", keys: ["mod+enter"], title: "Start the session", group: "New session", allowInInput: true, priority: 20, when: () => open, run: () => void submit() });
  const pending = useAppState((s) => !!s.pending[pk]) || !!step;
  const stepText = !step ? "" : step.step === "creating" ? "Starting the session…" : step.step === "sending" ? "Sending your message…" : `Uploading ${step.name} (${step.index} of ${step.total}) · ${Math.round((step.done / Math.max(1, step.size)) * 100)}%`;

  const changed = eff?.items.filter((i) => i.state !== "inherited").length ?? 0;
  const auto = eff?.items.filter((i) => i.state === "automatic").length ?? 0;
  const verb = onAdd ? "Add to design" : f.start === "continue" ? "Continue session" : "Start session";

  return (
    <Dialog open={open} onClose={onClose} title={title} subtitle={subtitle} width={960} className="nsdlg"
      footer={
        <>
          <span className="faint ns-problem" role="status">{stepText || problem || reach || (onAdd ? "The Wasl connector will be added automatically." : "")}</span>
          <span className="sp" />
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" icon={onAdd ? "plus" : f.start === "continue" ? "refresh" : "send"} disabled={!!problem} blocked={reach} pending={pending} onClick={() => void submit()}>
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
              <Select label="Model" value={f.model || undefined} placeholder={inv?.models.length ? "Select…" : "Reported after start"} options={modelOptions(inv?.models ?? [])} onChange={(model) => set({ model })} />
            </div>
            <div className="field"><span className="lbl">Mode</span>
              <Select label="Mode" value={f.mode || undefined} placeholder={inv?.modes.length ? "Select…" : "Reported after start"} options={(inv?.modes ?? []).map((m) => ({ value: m.id, label: m.label, description: m.description }))} onChange={(mode) => set({ mode })} />
            </div>
          </div>
          <span className="disc">
            <Icon name="refresh" size="xs" />
            {error ? <span className="c-failed">{error}</span> : inv ? `Models and modes discovered from ${inv.command ? `${inv.command} (this host's command for ${inv.display_name})` : inv.binary_path?.split(/[\\/]/).pop() ?? inv.driver} ${inv.version ?? ""} on ${host?.name ?? f.host} · ${inv.models.length} models, ${inv.modes.length} modes${loading ? " · refreshing…" : ""}` : loading ? "Asking the host what is installed…" : ""}
          </span>
          {inv?.warning ? <span className="disc c-needs" role="status"><Icon name="warn" size="xs" />{inv.warning}</span> : null}
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
            <div
              className={`ns-prompt${drag ? " drop" : ""}`}
              onDragOver={(e) => {
                if (!canAttach || !e.dataTransfer?.types.includes("Files")) return;
                e.preventDefault();
                setDrag(true);
              }}
              onDragLeave={() => setDrag(false)}
              onDrop={(e) => {
                setDrag(false);
                if (!canAttach || !e.dataTransfer?.files.length) return;
                e.preventDefault();
                addFiles(Array.from(e.dataTransfer.files).map(fromFile));
              }}
            >
              {promptInput ? <PendingList files={files} input={promptInput} onRemove={removeFile} /> : null}
              <textarea id="ns-prompt" className="ta" rows={f.start === "continue" ? 2 : 6} value={f.prompt} onChange={(e) => set({ prompt: e.target.value })}
                onPaste={(e) => {
                  const pasted = Array.from(e.clipboardData?.files ?? []);
                  if (!pasted.length || !canAttach) return;
                  e.preventDefault();
                  addFiles(pasted.map(fromFile));
                }}
                placeholder={f.start === "continue" ? "Pick up where you left off…" : "What should the agent do?"} />
              {onAdd ? null : (
                <div className="ns-attach">
                  <input ref={fileInput} type="file" multiple hidden data-testid="ns-attach-input" onChange={(e) => { addFiles(Array.from(e.target.files ?? []).map(fromFile)); e.target.value = ""; }} />
                  <Button size="sm" icon="clip" disabled={!canAttach || !!step} onClick={() => void attach()} aria-label="Attach files">Attach</Button>
                  <span className="faint fs12">{!promptInput ? "" : !canAttach ? "This agent takes no attachments." : "Paste, drop or attach images or files."}</span>
                </div>
              )}
              {dropped.length ? <div className="comp-msg" role="status" data-testid="ns-dropped">Removed because the selected agent cannot take {dropped.length === 1 ? "it" : "them"}: {dropped.join(" · ")}</div> : null}
            </div>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
