import { permissionShort } from "../../lib/gatewords";
import { useEffect, useMemo, useState } from "react";
import { money } from "../../lib/format";
import { Link, sessionPath, useNavigate } from "../../router";
import { isTyping } from "../../shortcuts/registry";
import { useShortcut } from "../../shortcuts/useShortcut";
import { backgroundWords, canInterrupt, endPendingKey, escapeInterrupts, interruptPendingKey, interruptWords, notContinuableWords, phaseWord, selectView, sessionVersionState, useActions, useApi, useApp, useAppState, useDeveloperMode, useDriver, useHost, usePending, useSessionReach, useSessionView } from "../../state";
import { Button, ConfirmDialog, ConnState, DriverBadge, EmptyState, ErrorBoundary, HostChip, Icon, IconButton, Skeleton, StatusPill, Tabs, useEscape } from "../../ui";
import { toast } from "../../ui/toast";
import { ContinueButton } from "../common/SessionBits";
import { Gate } from "../gates/Gate";
import { Composer } from "./Composer";
import { CompactButton, ForkButton } from "./SessionActions";
import { ForkedBanner } from "./ForkedBanner";
import { MediaCtx } from "../media/Parts";
import { SessionFacts } from "./SessionFacts";
import { SessionWork } from "./SessionWork";
import { SubagentStrip, type SubagentFilter } from "./SubagentStrip";
import { SubagentFoot, SubagentView } from "./SubagentView";
import { TranscriptList } from "./TranscriptList";
import { MAIN } from "../../state";
import { usageSummary } from "./usage";
import { ChangesTab } from "./tabs/ChangesTab";
import { ConfigTab } from "./tabs/ConfigTab";
import { SessionDiagnostics } from "../diagnostics/SessionDiagnostics";
import { TerminalTab } from "./tabs/TerminalTab";
import "./session.css";
import "./tabs/tabs.css";

const TABS = [
  { id: "activity", label: "Activity" },
  { id: "changes", label: "Changes" },
  { id: "config", label: "Config" },
  { id: "terminal", label: "Terminal" },
];
/** Developer mode adds this one (and takes it away again when it is switched off). */
const DIAGNOSTICS_TAB = { id: "diagnostics", label: "Diagnostics" };

function gateSummary(kinds: string[]): string {
  const c = (k: string, w: string) => (kinds.filter((x) => x === k).length ? `${kinds.filter((x) => x === k).length} ${w}${kinds.filter((x) => x === k).length > 1 ? "s" : ""}` : "");
  return [c("permission", "permission"), c("question", "question"), c("plan", "plan"), c("handoff", "handoff"), c("spawn", "spawn request")].filter(Boolean).join(", ");
}

/** `sub`: the call id of the subagent whose transcript is open in Activity (instead of the session's own). */
export function SessionPage({ host, id, tab: tabParam, sub }: { host: string; id: string; tab: string; sub?: string }) {
  // An address that names a section this version does not have (an old `?tab=usage` link, or Diagnostics with Developer mode off) opens the first one.
  const developer = useDeveloperMode();
  const tabs = developer ? [...TABS, DIAGNOSTICS_TAB] : TABS;
  const tab = tabs.some((t) => t.id === tabParam) ? tabParam : "activity";
  const key = `${host}/${id}`;
  const v = useSessionView(key);
  const snap = useAppState((s) => s.sessions[key]);
  const hostLink = useHost(host);
  const reach = useSessionReach(v);
  const drv = useDriver(v?.driver ?? "");
  const nav = useNavigate();
  const actions = useActions();
  const { store } = useApp();
  const loaded = useAppState((s) => s.fleetReady);
  const media = useMemo(() => ({ host, session: id, localFiles: !!hostLink?.localFiles, distro: hostLink?.distro ?? null, windows: hostLink?.os === "windows" }), [host, id, hostLink?.localFiles, hostLink?.distro, hostLink?.os]);

  const api = useApi();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [subStripOpen, setSubStripOpen] = useState(false);
  const [subStripFilter, setSubStripFilter] = useState<SubagentFilter | null>(null);
  const saveName = () => {
    const name = renaming?.trim() ?? "";
    setRenaming(null);
    if (name === (snap?.display_name ?? "")) return;
    void actions.run(`rename:${key}`, "Could not rename the session", () => api.renameSession(host, id, name || null));
  };
  const remove = async () => {
    setConfirmDelete(false);
    const r = await actions.run(`delete:${key}`, "Could not delete the session", () => api.deleteSession(host, id));
    if (r) {
      toast({ tone: "ok", title: "Session deleted", detail: v?.name });
      nav("/inbox");
    }
  };
  // Looking at a finished session counts as seeing it (once the host is reporting it live).
  useEffect(() => {
    const view = selectView(store.get(), key);
    if (view?.status === "done" && !view.stale) actions.markSeen([key]);
  }, [key, v?.status, v?.stale, store, actions]);
  useEscape(renaming != null, () => setRenaming(null));
  // Esc, innermost first: an open menu, dialog, preview or editor takes it (the escape stack, before these run). Then, with
  // a turn running, it interrupts that turn and nothing else (never background work: see `escapeInterrupts`); when none
  // is, it leaves the text field it is in, and once nothing has the focus it goes back (from a subagent's transcript to the
  // session, else to the Inbox). One key press does one of these.
  // In the Terminal tab Esc belongs to the shell (vim, readline): none of these see it there.
  const interruptPending = usePending(interruptPendingKey(key)) === "pending";
  const inTerminal = () => !!document.activeElement?.closest(".xterm-box");
  useShortcut({ id: "session-interrupt", keys: ["escape"], title: "Interrupt the turn that is running", group: "Session", allowInInput: true, priority: 2,
    when: () => !inTerminal() && !!snap && escapeInterrupts(snap), run: () => { if (!interruptPending) void actions.interrupt(key); } });
  useShortcut({ id: "session-blur", keys: ["escape"], title: "Leave the text field you are typing in", group: "Session", allowInInput: true, priority: 1,
    when: () => !inTerminal() && isTyping(document.activeElement), run: () => (document.activeElement as HTMLElement | null)?.blur() });
  useShortcut({ id: "session-back", keys: ["escape"], title: "Back to the Inbox, or from a subagent's transcript to the session (when no turn is running)", group: "Session", run: () => nav(sub && tab === "activity" ? sessionPath(host, id) : "/inbox") });
  const endPending = usePending(endPendingKey(key)) === "pending";

  if (!snap || !v) {
    return (
      <div className="placeholder">
        {loaded ? <EmptyState icon="inbox" title="Session not found" action={<Button size="sm" onClick={() => nav("/inbox")}>Back to the Inbox</Button>}>It may have been deleted, or it lives on a host that has not reported yet.</EmptyState>
          : <div className="skelbox"><Skeleton width="60%" height={18} /><Skeleton /><Skeleton width="80%" /></div>}
      </div>
    );
  }
  const gates = snap.pending_gates;
  const plan = snap.plan;
  const doneN = plan.filter((p) => p.status === "completed").length;
  const usage = usageSummary(snap);
  const mins = Math.max(1, Math.round((Date.now() - snap.created_at_ms) / 60000));
  const pillLabel = v.status === "needs" && gates.length ? `Waiting for you · ${gateSummary(gates.map((g) => g.gate.gate_type))}` : v.backgroundOnly ? backgroundWords(v.backgroundOnly) : phaseWord(v);
  const cont = v.cont;

  return (
    <>
      <div className="sd-head">
        <nav className="crumbs" aria-label="Breadcrumb">
          <Link to="/inbox"><Icon name="inbox" size="xs" /> Inbox</Link>
          <Icon name="chevr" size="xs" />
          <span>{v.project}</span>
          <Icon name="chevr" size="xs" />
          <span className="fg" aria-current="page">{v.name}</span>
        </nav>
        <div className="sd-title">
          {renaming != null ? (
            <input className="h1 rename" aria-label="Session name" autoFocus value={renaming} placeholder={snap.title ?? id} onChange={(e) => setRenaming(e.target.value)} onBlur={saveName}
              onKeyDown={(e) => { if (e.key === "Enter") saveName(); }} />
          ) : (
            <h1 className="h1" title={snap.display_name ? `First prompt: ${snap.title ?? ""}` : undefined}>{v.name}</h1>
          )}
          {renaming == null ? <IconButton icon="edit" label="Rename session" disabled={!!reach} title={reach ?? "Rename session"} onClick={() => setRenaming(snap.display_name ?? "")} /> : null}
          <StatusPill status={v.status} label={pillLabel} stale={v.stale} />{v.stale ? <span className="lk">last known</span> : null}
          <span className="flex1" />
          {cont.kind === "running" ? (
            <>
              <Button size="sm" icon="pause" pending={interruptPending} blocked={reach} disabled={!canInterrupt(snap)} title={interruptWords(snap)} onClick={() => void actions.interrupt(key)}>Interrupt</Button>
              <Button size="sm" icon="stop" pending={endPending} blocked={reach} title="Shut the agent down. The session is kept and can be continued." onClick={() => void actions.endSession(key)}>End session</Button>
              <CompactButton sessionKey={key} snap={snap} blocked={reach} />
              <ForkButton host={host} sessionKey={key} snap={snap} blocked={reach} />
            </>
          ) : (
            <>
              <ContinueButton view={v} />
              <Button size="sm" variant="ghost" icon="x" blocked={reach} onClick={() => setConfirmDelete(true)}>Delete</Button>
            </>
          )}
        </div>
        {cont.kind === "blocked" ? <div className="sd-ended" role="status" data-testid="continue-why"><Icon name="stop" size="sm" />{notContinuableWords(cont.why)}</div> : null}
        <div className="sd-meta">
          <span className="kv">{drv ? <DriverBadge driver={drv} /> : v.driver}{snap.cli_version ? <span className="faint mono">v{snap.cli_version}</span> : sessionVersionState(snap, v.stale) === "loading" ? <span className="faint small" role="status" data-testid="session-version-loading">loading…</span> : null}</span>
          {hostLink ? <span className="kv"><HostChip host={hostLink} /><ConnState host={hostLink} /></span> : null}
          {snap.cwd ? <span className="kv"><Icon name="folder" size="sm" /><span className="mono">{snap.cwd}</span></span> : null}
          {v.branch ? <span className="kv"><Icon name="branch" size="sm" /><span className="mono">{v.branch}</span></span> : null}
          {usage.contextPct != null ? (
            <span className="kv ctx">Context <span className="bar"><i style={{ width: `${Math.round(usage.contextPct)}%` }} /></span><span className="tnum">{Math.round(usage.contextPct)}%</span></span>
          ) : null}
          {snap.background_running > 0 && !v.backgroundOnly ? <span className="kv" data-testid="background-running"><Icon name="working" size="sm" spin className="c-working" />{snap.background_running} background task{snap.background_running === 1 ? "" : "s"} running</span> : null}
          <span className="kv"><Icon name="coin" size="sm" /><span className="tnum">{usage.reportsCost ? money(usage.cost) : "n/a"}</span><span className="faint">· {mins >= 60 ? `${Math.floor(mins / 60)}h ${mins % 60}m` : `${mins}m`}</span></span>
        </div>
        {v.stale ? <div className="banner warn sd-stale" role="status" data-testid="stale-note"><Icon name="clock" size="sm" /><div className="bt"><b>Last known state.</b> <span>{reach}</span></div></div> : null}
        <ForkedBanner host={host} snap={snap} />
        <Tabs tabs={tabs} value={tab} label="Session sections" onChange={(t) => nav(sessionPath(host, id, t))} />
      </div>
      {tab !== "activity" ? (
        <div className="tabbody" role="tabpanel" aria-label={tabs.find((t) => t.id === tab)?.label}>
          {tab === "changes" ? <ChangesTab sessionKey={key} /> : tab === "config" ? <ConfigTab sessionKey={key} snap={snap} /> : tab === "diagnostics" ? <ErrorBoundary variant="item" name="session_diagnostics" what="the diagnostics" resetKey={key}><SessionDiagnostics key={key} sessionKey={key} snap={snap} /></ErrorBoundary> : <TerminalTab key={key} sessionKey={key} />}
        </div>
      ) : (
        <div className="sd-body">
          <div className="tr-wrap">
            <MediaCtx.Provider value={media}>
              {sub ? (
                <SubagentView sessionKey={key} host={host} id={id} callId={sub} snap={snap} driver={drv} />
              ) : (
                <TranscriptList sessionKey={key} host={host} id={id} scope={MAIN} driver={drv} followKey={gates.length}>
                  {gates.map((pg, i) => (
                    <div key={pg.gate.request_id} id={`gate-${pg.gate.request_id}`}>
                      <Gate view={v} pg={pg} density="full" active={i === 0} />
                    </div>
                  ))}
                </TranscriptList>
              )}
              <SubagentStrip
                sessionKey={key}
                host={host}
                id={id}
                snap={snap}
                selected={sub}
                open={subStripOpen}
                onOpenChange={setSubStripOpen}
                filter={subStripFilter}
                onFilterChange={setSubStripFilter}
              />
              {sub ? <SubagentFoot snap={snap} callId={sub} /> : <Composer sessionKey={key} snap={snap} />}
            </MediaCtx.Provider>
          </div>
          <aside className="side" aria-label="Session details">
            {gates.length ? (
              <div>
                <h4>Waiting for you</h4>
                <div className="jump">
                  {gates.map((pg) => (
                    <a key={pg.gate.request_id} href={`#gate-${pg.gate.request_id}`} onClick={(e) => { e.preventDefault(); if (sub) nav(sessionPath(host, id)); else document.getElementById(`gate-${pg.gate.request_id}`)?.scrollIntoView({ block: "center" }); }}>
                      <Icon name={pg.gate.gate_type === "permission" ? "shield" : pg.gate.gate_type === "handoff" ? "handoff" : pg.gate.gate_type === "spawn" ? "sparkle" : "needs"} size="sm" className="c-needs" />
                      {pg.gate.gate_type === "permission" ? permissionShort(pg.gate) : pg.gate.gate_type === "question" ? pg.gate.questions[0]?.header || "Question" : pg.gate.gate_type === "handoff" ? "Handoff" : pg.gate.gate_type === "spawn" ? `Start ${pg.gate.name}` : "Plan"}
                    </a>
                  ))}
                </div>
              </div>
            ) : null}
            {plan.length ? (
              <div>
                <h4>Plan · {doneN} of {plan.length}</h4>
                <div className="todo">
                  {plan.map((p, i) => (
                    <div key={i} className={p.status === "completed" ? "dn" : p.status === "in_progress" ? "" : "muted"}>
                      <Icon name={p.status === "completed" ? "done" : p.status === "in_progress" ? "needs" : "idle"} size="sm" className={p.status === "completed" ? "c-done" : p.status === "in_progress" ? "c-needs" : ""} />
                      {p.content}
                    </div>
                  ))}
                </div>
              </div>
            ) : null}
            <SessionWork sessionKey={key} snap={snap} />
            <SessionFacts snap={snap} />
          </aside>
        </div>
      )}
      <ConfirmDialog open={confirmDelete} title={`Delete ${v.name}?`} confirmLabel="Delete session" danger onConfirm={() => void remove()} onClose={() => setConfirmDelete(false)}>
        Removes the session and its transcript from {hostLink?.name ?? host}. The agent's own conversation files are kept, so it can still be resumed from the agent's history.
      </ConfirmDialog>
    </>
  );
}
