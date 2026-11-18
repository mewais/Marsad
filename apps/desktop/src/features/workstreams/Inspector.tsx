import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { EdgePolicy, TaskView, WorkstreamView } from "../../bridge/wire";
import { sessionKey } from "../../bridge/wire";
import { clockTime } from "../../lib/format";
import { sessionPath, useNavigate } from "../../router";
import { linkView, useActions, useApi, useAppState, useHost, useSessionView } from "../../state";
import { Button, ConnState, DriverBadge, HostChip, Icon, Select, makeDriver } from "../../ui";
import { Gate } from "../gates/Gate";
import { checklist, nodeName, nodeStatus, POLICY, taskWords } from "./model";
import { PendingSpawn, SpawnSection } from "./Spawning";

function PolicyPick({ value, onChange, busy }: { value: EdgePolicy; onChange: (p: EdgePolicy) => void; busy: boolean }) {
  return (
    <div className="pol" role="radiogroup" aria-label="Policy for this connection">
      {(Object.keys(POLICY) as EdgePolicy[]).map((p) => (
        <button key={p} type="button" role="radio" aria-checked={p === value} disabled={busy} className={`po${p === value ? " on" : ""}`} onClick={() => p !== value && onChange(p)}>
          <Icon name={POLICY[p].icon} size="sm" className={`c-${p === "ask" ? "needs" : p === "always_allow" ? "done" : "failed"}`} /><span><b>{POLICY[p].word}</b><span className="d">{POLICY[p].desc}</span></span>
        </button>
      ))}
    </div>
  );
}

const taskIcon = (t: TaskView) => (t.state.state === "completed" ? "done" : t.state.state === "denied" || t.state.state === "failed" ? "ban" : t.state.state === "pending_approval" ? "needs" : "working");
const taskCls = (t: TaskView) => (t.state.state === "completed" ? "c-done" : t.state.state === "denied" || t.state.state === "failed" ? "c-failed" : t.state.state === "pending_approval" ? "c-needs" : "c-working");

export function EdgeInspector({ view, from, to, design }: { view: WorkstreamView; from: string; to: string; design: boolean }) {
  const api = useApi();
  const actions = useActions();
  const e = view.edges.find((x) => x.from === from && x.to === to);
  const [hist, setHist] = useState<TaskView[] | null>(null);
  const [err, setErr] = useState("");
  const id = view.workstream.id;
  const fromHost = view.nodes.find((n) => n.node === from)?.host ?? "";
  const toHost = view.nodes.find((n) => n.node === to)?.host ?? "";
  const fh = useHost(fromHost);
  const th = useHost(toHost);
  useEffect(() => {
    if (design) return;
    let off = false;
    setHist(null);
    api.ws.history({ workstream: id, from, to, limit: 20 }).then((p) => !off && setHist(p.tasks), (x) => !off && setErr(errorMessage(x)));
    return () => {
      off = true;
    };
  }, [api, id, from, to, design, e?.completed, e?.in_flight.length]);
  const gate = e?.pending_gates[0];
  const senderKey = gate ? sessionKey(gate.host, gate.session) : "";
  const sender = useSessionView(senderKey);
  const busy = !!useAppState((s) => s.pending[`policy:${id}:${from}:${to}`]);
  if (!e) return null;
  const setPolicy = (policy: EdgePolicy) => void actions.run(`policy:${id}:${from}:${to}`, "Could not change the policy", () => api.ws.setPolicy({ workstream: id, from, to, policy }));
  return (
    <aside className="insp" aria-label="Connection">
      <div className="sec">
        <div className="eyebrow">Connection</div>
        <div className="conn-names">
          <span><b>{nodeName(view, from)}</b> <span className="faint">{fh?.name ?? fromHost}</span></span>
          <span className="faint"><Icon name="arrowdown" size="xs" /> {design ? "can hand work to" : "delegates to"}</span>
          <span><b>{nodeName(view, to)}</b> <span className="faint">{th?.name ?? toHost}</span>{th && !linkView(th.link, th.authPrompt).connected ? <> <ConnState host={th} short /></> : null}</span>
        </div>
      </div>
      {gate && sender ? (
        <div className="sec" data-testid="edge-gate">
          <Gate view={sender} pg={{ gate: gate.gate, raised_at_ms: gate.raised_at_ms, cursor: 0 }} density="full" active />
        </div>
      ) : null}
      <div className="sec"><div className="eyebrow">Policy for this connection</div><PolicyPick value={e.policy} onChange={setPolicy} busy={busy} /></div>
      {e.in_flight.length ? (
        <div className="sec"><div className="eyebrow">In flight</div><div className="hist">
          {e.in_flight.map((t) => <div key={t.task.id} className="h"><Icon name={taskIcon(t)} size="sm" className={taskCls(t)} spin={t.state.state === "running"} /><div>{t.task.instruction}<div className="d">{taskWords(t)}</div></div><span className="w">{clockTime(t.task.created_ms)}</span></div>)}
        </div></div>
      ) : null}
      {!design ? (
        <div className="sec"><div className="eyebrow">Handoff history · {e.completed} finished, {e.unsuccessful} not</div>
          {err ? <span className="c-failed small">{err}</span> : !hist ? <span className="faint small">Loading…</span> : hist.length === 0 ? <span className="faint small">No handoffs over this connection yet.</span> : (
            <div className="hist">{hist.map((t) => <div key={t.task.id} className="h"><Icon name={taskIcon(t)} size="sm" className={taskCls(t)} /><div>{t.task.instruction}<div className="d">{taskWords(t)}</div></div><span className="w">{clockTime(t.task.created_ms)}</span></div>)}</div>
          )}
        </div>
      ) : (
        <Button size="sm" variant="ghost" icon="x" onClick={() => void actions.run(`disc:${id}`, "Could not remove the connection", () => api.ws.disconnect({ workstream: id, from, to }))}>Remove connection</Button>
      )}
    </aside>
  );
}

export function NodeInspector({ view, nodeId, design }: { view: WorkstreamView; nodeId: string; design: boolean }) {
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const node = view.workstream.nodes.find((n) => n.id === nodeId);
  const nv = view.nodes.find((n) => n.node === nodeId);
  const host = useHost(nv?.host ?? "");
  const drivers = useAppState((s) => s.drivers);
  const [target, setTarget] = useState("");
  if (!node || !nv) return null;
  const snap = nv.session;
  const st = nodeStatus(nv, snap);
  const drvId = snap?.driver ?? node.launch?.driver ?? "";
  const id = view.workstream.id;
  const others = view.workstream.nodes.filter((n) => n.id !== nodeId && !view.workstream.edges.some((e) => e.from === nodeId && e.to === n.id));
  const conns = view.edges.filter((e) => e.from === nodeId || e.to === nodeId);
  return (
    <aside className="insp" aria-label="Agent">
      <div className="sec">
        <div className="eyebrow">Agent</div>
        <div className="h2">{node.name}</div>
        <div className="row-gap"><span className={`st ${st.status}`}>{st.word}</span>{drvId ? <DriverBadge driver={drivers[drvId] ?? makeDriver(drvId)} /> : null}</div>
        {host ? <div className="hostline"><HostChip host={host} /><ConnState host={host} /></div> : null}
        <div className="mono faint small">{snap?.cwd ?? node.launch?.cwd ?? ""}</div>
      </div>
      {node.spawned_by ? <div className="sec"><div className="eyebrow">Spawned</div><div className="small"><Icon name="sparkle" size="xs" className="c-accent" /> Started by <b>{nodeName(view, node.spawned_by)}</b>{node.role ? <> to {node.role.charAt(0).toLowerCase() + node.role.slice(1)}</> : null}</div></div> : null}
      <PendingSpawn view={view} node={nodeId} />
      <div className="sec"><div className="eyebrow">Now</div><div className="small">{snap ? snap.turn?.prompt ?? snap.last_turn?.reply ?? st.word : node.launch?.first_prompt ? `Will start with: “${node.launch.first_prompt}”` : "Waits for a handoff once launched."}</div></div>
      <div className="sec"><div className="eyebrow">Connections</div>
        {conns.length ? <div className="hist">{conns.map((e) => <div key={`${e.from}>${e.to}`} className="h"><Icon name={POLICY[e.policy].icon} size="sm" className={`c-${e.policy === "ask" ? "needs" : e.policy === "always_allow" ? "done" : "failed"}`} /><span>{nodeName(view, e.from)} → {nodeName(view, e.to)}</span><span className="w">{POLICY[e.policy].word}</span></div>)}</div> : <span className="faint small">Not connected yet.</span>}
        {design && others.length ? (
          <div className="row-gap">
            <Select label="Connect to" value={target || undefined} placeholder="Connect to…" options={others.map((n) => ({ value: n.id, label: n.name }))} onChange={setTarget} />
            <Button size="sm" disabled={!target} onClick={() => { void actions.run(`conn:${id}`, "Could not connect", () => api.ws.connect({ workstream: id, from: nodeId, to: target, policy: "ask" })); setTarget(""); }}>Connect</Button>
          </div>
        ) : null}
      </div>
      <div className="row-gap">
        {snap ? <Button size="sm" variant="primary" onClick={() => nav(sessionPath(nv.host, snap.id))}>Open session</Button> : null}
        {snap ? <Button size="sm" onClick={() => nav(`/config?driver=${encodeURIComponent(snap.driver)}&host=${encodeURIComponent(nv.host)}&session=${encodeURIComponent(snap.id)}`)}>Edit node config</Button> : null}
        <Button size="sm" variant="ghost" onClick={() => void actions.run(`rm:${id}:${nodeId}`, "Could not remove the agent", () => api.ws.removeNode(id, nodeId))}>Remove from workstream</Button>
      </div>
    </aside>
  );
}

/** Nothing selected in a running workstream: a waiting spawn request, and the spawning rules. */
export function WorkstreamPanel({ view }: { view: WorkstreamView }) {
  return (
    <aside className="insp" aria-label="Workstream">
      <PendingSpawn view={view} />
      <SpawnSection view={view} />
      <div className="faint small">Select an agent or a connection.</div>
    </aside>
  );
}

export function DesignPanel({ view, onLaunch, onSave, launching }: { view: WorkstreamView; onLaunch: () => void; onSave: () => void; launching: boolean }) {
  const hosts = useAppState((s) => s.hosts);
  const items = checklist(view, (h) => !!hosts[h] && linkView(hosts[h].link, hosts[h].authPrompt).connected, (h) => hosts[h]?.entry.name ?? h);
  const n = view.workstream.nodes.length;
  const toLaunch = view.workstream.nodes.filter((x) => !x.session).length;
  return (
    <aside className="insp" aria-label="Design">
      <div className="summ"><Icon name="check" size="sm" /><span>{n} agent{n === 1 ? "" : "s"} · {view.workstream.edges.length} connection{view.workstream.edges.length === 1 ? "" : "s"} · Wasl connector will be added to each</span></div>
      <div className="sec"><div className="eyebrow">Before you launch</div>
        <div className="cl">{items.map((c, i) => <div key={i} className="ci"><Icon name={c.ok === "done" ? "done" : c.ok === "info" ? "lock" : c.ok === "bad" ? "failed" : "warn"} size="sm" className={c.ok === "done" ? "c-done" : c.ok === "info" ? "c-accent" : c.ok === "bad" ? "c-failed" : "c-needs"} /><div>{c.text}<div className="d">{c.detail}</div></div></div>)}</div>
      </div>
      <SpawnSection view={view} />
      <div className="faint small">Select an agent or a connection to edit it. Drag agents to move them; drag from an agent's dot to connect.</div>
      <div className="insp-foot"><Button size="sm" onClick={onSave}>Save as draft</Button><span className="sp" /><Button size="sm" variant="primary" icon="send" disabled={!n} pending={launching} onClick={onLaunch}>Launch {toLaunch || n} agent{(toLaunch || n) === 1 ? "" : "s"}</Button></div>
    </aside>
  );
}
