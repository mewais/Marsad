import { useEffect, useMemo, useRef, useState } from "react";
import { clockTime, compactNumber, money } from "../../lib/format";
import { Link, sessionPath, useNavigate } from "../../router";
import { useShortcut } from "../../shortcuts/useShortcut";
import { selectView, useActions, useApi, useApp, useAppState, useDriver, useHost, useSessionView, useTranscriptOpen } from "../../state";
import { Button, ConfirmDialog, ConnState, DriverBadge, EmptyState, HostChip, Icon, IconButton, Skeleton, StatusPill, Tabs } from "../../ui";
import { toast } from "../../ui/toast";
import { Gate } from "../gates/Gate";
import { Composer } from "./Composer";
import { TranscriptItem } from "./TranscriptView";
import { buildItems } from "./transcript";
import { ChangesTab } from "./tabs/ChangesTab";
import { ConfigTab } from "./tabs/ConfigTab";
import { TerminalTab } from "./tabs/TerminalTab";
import { UsageTab } from "./tabs/UsageTab";
import "./session.css";
import "./tabs/tabs.css";

const TABS = [
  { id: "activity", label: "Activity" },
  { id: "changes", label: "Changes" },
  { id: "config", label: "Config" },
  { id: "usage", label: "Usage" },
  { id: "terminal", label: "Terminal" },
];

function gateSummary(kinds: string[]): string {
  const c = (k: string, w: string) => (kinds.filter((x) => x === k).length ? `${kinds.filter((x) => x === k).length} ${w}${kinds.filter((x) => x === k).length > 1 ? "s" : ""}` : "");
  return [c("permission", "permission"), c("question", "question"), c("plan", "plan"), c("handoff", "handoff")].filter(Boolean).join(", ");
}

export function SessionPage({ host, id, tab }: { host: string; id: string; tab: string }) {
  const key = `${host}/${id}`;
  const v = useSessionView(key);
  const snap = useAppState((s) => s.sessions[key]);
  const hostLink = useHost(host);
  const drv = useDriver(v?.driver ?? "");
  const nav = useNavigate();
  const actions = useActions();
  const { store, transcripts } = useApp();
  const loaded = useAppState((s) => s.fleetReady);
  useTranscriptOpen(key);
  const slice = useAppState((s) => s.transcripts[key]);
  const items = useMemo(() => buildItems(slice?.events ?? []), [slice?.events]);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const api = useApi();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
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
  // Looking at a finished session counts as seeing it.
  useEffect(() => {
    if (selectView(store.get(), key)?.status === "done") actions.markSeen([key]);
  }, [key, v?.status, store, actions]);
  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items.length, snap?.pending_gates.length]);
  useShortcut({ id: "session-back", keys: ["escape"], title: "Back to the Inbox", group: "Session", run: () => nav("/inbox") });

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
  const u = snap.usage;
  const cachePct = u.input_tokens + u.cache_read_tokens ? Math.round((u.cache_read_tokens / (u.input_tokens + u.cache_read_tokens)) * 100) : 0;
  const mins = Math.max(1, Math.round((Date.now() - snap.created_at_ms) / 60000));
  const pillLabel = v.status === "needs" && gates.length ? `Waiting for you · ${gateSummary(gates.map((g) => g.gate.gate_type))}` : undefined;
  const live = snap.phase.state === "working" || snap.phase.state === "starting" || snap.phase.state === "needs_you";

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
              onKeyDown={(e) => { if (e.key === "Enter") saveName(); else if (e.key === "Escape") { e.stopPropagation(); setRenaming(null); } }} />
          ) : (
            <h1 className="h1" title={snap.display_name ? `First prompt: ${snap.title ?? ""}` : undefined}>{v.name}</h1>
          )}
          {renaming == null ? <IconButton icon="edit" label="Rename session" onClick={() => setRenaming(snap.display_name ?? "")} /> : null}
          <StatusPill status={v.status} label={pillLabel} />
          <span className="flex1" />
          <Button size="sm" icon="pause" disabled={!live || !snap.capabilities.interrupt} onClick={() => void actions.stop(key)}>Interrupt</Button>
          {snap.phase.state === "exited" || snap.phase.state === "failed" || snap.phase.state === "lost" ? (
            <Button size="sm" variant="ghost" icon="x" onClick={() => setConfirmDelete(true)}>Delete</Button>
          ) : (
            <Button size="sm" icon="stop" onClick={() => void actions.stopSession(key)}>Stop</Button>
          )}
        </div>
        <div className="sd-meta">
          <span className="kv">{drv ? <DriverBadge driver={drv} /> : v.driver}{snap.cli_version ? <span className="faint mono">v{snap.cli_version}</span> : null}</span>
          {hostLink ? <span className="kv"><HostChip host={hostLink} /><ConnState host={hostLink} /></span> : null}
          {snap.cwd ? <span className="kv"><Icon name="folder" size="sm" /><span className="mono">{snap.cwd}</span></span> : null}
          {v.branch ? <span className="kv"><Icon name="branch" size="sm" /><span className="mono">{v.branch}</span></span> : null}
          {snap.context_pct != null ? (
            <span className="kv ctx">Context <span className="bar"><i style={{ width: `${Math.round(snap.context_pct)}%` }} /></span><span className="tnum">{Math.round(snap.context_pct)}%</span></span>
          ) : null}
          <span className="kv"><Icon name="coin" size="sm" /><span className="tnum">{money(u.cost_usd)}</span><span className="faint">· {mins >= 60 ? `${Math.floor(mins / 60)}h ${mins % 60}m` : `${mins}m`}</span></span>
        </div>
        <Tabs tabs={TABS} value={tab} label="Session sections" onChange={(t) => nav(sessionPath(host, id, t))} />
      </div>
      {tab !== "activity" ? (
        <div className="tabbody" role="tabpanel" aria-label={TABS.find((t) => t.id === tab)?.label}>
          {tab === "changes" ? <ChangesTab sessionKey={key} /> : tab === "config" ? <ConfigTab sessionKey={key} snap={snap} /> : tab === "usage" ? <UsageTab sessionKey={key} snap={snap} /> : <TerminalTab key={key} sessionKey={key} />}
        </div>
      ) : (
        <div className="sd-body">
          <div className="tr-wrap">
            <div className="transcript" ref={scroller} onScroll={(e) => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80; }}>
              <div className="tr">
                {slice?.hasMore ? <Button size="sm" variant="ghost" pending={slice.loading} onClick={() => void transcripts.loadEarlier(key)}>Load earlier messages</Button> : null}
                {!slice || (slice.loading && items.length === 0) ? <><Skeleton width="50%" /><Skeleton /><Skeleton width="70%" /></> : null}
                {slice?.error ? <div className="c-failed" role="alert">{slice.error}</div> : null}
                {items.map((it) => <TranscriptItem key={it.key} item={it} driver={drv} />)}
                {gates.map((pg, i) => (
                  <div key={pg.gate.request_id} id={`gate-${pg.gate.request_id}`}>
                    <Gate view={v} pg={pg} density="full" active={i === 0} />
                  </div>
                ))}
              </div>
            </div>
            <Composer sessionKey={key} snap={snap} />
          </div>
          <aside className="side" aria-label="Session details">
            {gates.length ? (
              <div>
                <h4>Waiting for you</h4>
                <div className="jump">
                  {gates.map((pg) => (
                    <a key={pg.gate.request_id} href={`#gate-${pg.gate.request_id}`} onClick={(e) => { e.preventDefault(); document.getElementById(`gate-${pg.gate.request_id}`)?.scrollIntoView({ block: "center" }); }}>
                      <Icon name={pg.gate.gate_type === "permission" ? "shield" : pg.gate.gate_type === "handoff" ? "handoff" : "needs"} size="sm" className="c-needs" />
                      {pg.gate.gate_type === "permission" ? `Run ${pg.gate.command.slice(0, 28)}` : pg.gate.gate_type === "question" ? pg.gate.questions[0]?.header || "Question" : pg.gate.gate_type === "handoff" ? "Handoff" : "Plan"}
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
            <div>
              <h4>Session</h4>
              <dl className="kvl">
                <dt>Model</dt><dd className="mono">{snap.model ?? "n/a"}</dd>
                <dt>Mode</dt><dd>{snap.available_modes.find((m) => m.id === snap.mode)?.label ?? snap.mode ?? "n/a"}</dd>
                <dt>Tokens in / out</dt><dd>{compactNumber(u.input_tokens)} / {compactNumber(u.output_tokens)}</dd>
                <dt>Cache hits</dt><dd>{cachePct}%</dd>
                <dt>Started</dt><dd>{clockTime(snap.created_at_ms)}</dd>
              </dl>
            </div>
          </aside>
        </div>
      )}
      <ConfirmDialog open={confirmDelete} title={`Delete ${v.name}?`} confirmLabel="Delete session" danger onConfirm={() => void remove()} onClose={() => setConfirmDelete(false)}>
        Removes the session and its transcript from {hostLink?.name ?? host}. The agent's own conversation files are kept, so it can still be resumed from the agent's history.
      </ConfirmDialog>
    </>
  );
}
