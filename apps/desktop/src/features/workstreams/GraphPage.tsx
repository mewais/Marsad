import { useMemo, useState } from "react";
import type { DriverInventory, NodePosition, WorkstreamNode } from "../../bridge/wire";
import { clockTime, money } from "../../lib/format";
import { Link, useNavigate } from "../../router";
import { useShortcut } from "../../shortcuts/useShortcut";
import { linkView, useActions, useApi, useAppState, useHosts, usePending } from "../../state";
import { Button, EmptyState, Icon, toast } from "../../ui";
import { NewSessionDialog } from "../newsession/NewSessionDialog";
import { launchRequest, type NewSessionForm } from "../newsession/model";
import { Canvas, type Sel } from "./Canvas";
import { DesignPanel, EdgeInspector, NodeInspector } from "./Inspector";
import { summary } from "./model";
import { nodeId } from "./NewWorkstreamDialog";
import "./workstreams.css";

/** One workstream: host lanes and live agents (running), or the design canvas (draft). */
export function GraphPage({ id }: { id: string }) {
  const v = useAppState((s) => s.workstreams[id]);
  const loaded = useAppState((s) => s.workstreamsLoaded);
  const hosts = useHosts();
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const [sel, setSel] = useState<Sel>(null);
  const [adding, setAdding] = useState<{ host?: string } | null>(null);
  const launching = !!usePending(`launch:${id}`);
  const design = v?.workstream.state === "draft";
  const lanesFor = useMemo(() => {
    if (!v) return [];
    const used = new Set(v.nodes.map((n) => n.host));
    return hosts.filter((h) => used.has(h.id) || (design && h.link.link === "local") || (design && linkView(h.link, h.authPrompt).connected) || (design && h.link.link === "remote" && h.link.state.state === "reconnecting")).map((h) => h.id)
      // Lanes with agents first; empty ones (somewhere to add an agent) below them.
      .sort((a, b) => Number(used.has(b)) - Number(used.has(a)));
  }, [v, hosts, design]);

  useShortcut({ id: "ws-review", keys: ["r"], title: "Review the waiting handoff", group: "Workstream", when: () => !!v?.edges.some((e) => e.pending_gates.length), run: () => { const e = v!.edges.find((x) => x.pending_gates.length)!; setSel({ kind: "edge", from: e.from, to: e.to }); } });
  useShortcut({ id: "ws-add", keys: ["a"], title: "Add agent (design)", group: "Workstream", when: () => !!design, run: () => setAdding({}) });

  if (!v) return loaded ? <div className="placeholder"><EmptyState icon="graph" title="Workstream not found" action={<Button size="sm" onClick={() => nav("/workstreams")}>All workstreams</Button>}>It may have been deleted.</EmptyState></div> : <div className="placeholder faint">Loading…</div>;
  const w = v.workstream;
  const s = summary(v);

  const onConnect = (from: string, to: string) => {
    if (w.edges.some((e) => e.from === from && e.to === to)) return setSel({ kind: "edge", from, to });
    void actions.run(`conn:${id}`, "Could not connect the agents", () => api.ws.connect({ workstream: id, from, to, policy: "ask" })).then((r) => r && setSel({ kind: "edge", from, to }));
  };
  const onMove = (node: string, pos: NodePosition) => void actions.run(`move:${id}`, "Could not save the position", () => api.ws.update({ ...w, layout: { ...w.layout, [node]: pos } }, w.revision));
  const onAdd = (f: NewSessionForm, inv: DriverInventory) => {
    const taken = new Set(w.nodes.map((n) => n.id));
    const base = f.prompt.trim().split(/\s+/).slice(0, 3).join("-") || `${inv.driver}-agent`;
    const name = (() => { let n = base.toLowerCase().replace(/[^a-z0-9-]+/g, "-"); for (let i = 2; w.nodes.some((x) => x.name === n); i++) n = `${base}-${i}`; return n; })();
    const node: WorkstreamNode = { id: nodeId(name, taken), name, role: "", description: "", session: null, launch: { host: f.host, driver: f.driver, cwd: f.cwd || null, model: f.model || null, mode: f.mode || null, first_prompt: f.prompt.trim() || null, resume: launchRequest(f).resume } };
    void actions.run(`add:${id}`, "Could not add the agent", () => api.ws.addNode({ workstream: id, node, position: null })).then((r) => r && setSel({ kind: "node", id: node.id }));
  };
  const launch = async () => {
    const r = await actions.run(`launch:${id}`, "Could not launch the workstream", () => api.ws.launch(id));
    if (!r) return;
    const failed = r.launched.filter((x) => x.error);
    toast({ tone: failed.length ? "warn" : "ok", title: failed.length ? `${failed.length} agent${failed.length === 1 ? "" : "s"} did not start` : "Workstream launched", detail: failed.length ? failed.map((x) => `${x.node}: ${x.error && "message" in x.error ? x.error.message : x.error?.marsad_error}`).join("; ") : `${r.launched.length} agent${r.launched.length === 1 ? "" : "s"} started; handoffs go through Wasl.` });
  };
  const saveDraft = () => void actions.run(`save:${id}`, "Could not save the draft", () => api.ws.update(w, w.revision)).then((r) => r && toast({ tone: "ok", title: "Draft saved" }));
  const pauseResume = () => void actions.run(`pause:${id}`, w.state === "paused" ? "Could not resume" : "Could not pause", () => (w.state === "paused" ? api.ws.resume(id) : api.ws.pause(id)));

  return (
    <>
      <div className="ph ph-graph">
        <nav className="crumbs" aria-label="Breadcrumb"><Link to="/workstreams"><Icon name="graph" size="xs" /> Workstreams</Link><Icon name="chevr" size="xs" /><span aria-current="page" className="fg">{w.name}</span></nav>
        <h1 className="h1">{w.name}</h1>
        {design ? <span className="st draft"><Icon name="edit" size="sm" />Design · not launched</span> : s.parts.map((p) => <span key={p.text} className={`st ${p.cls}`}><Icon name={p.icon} size="sm" spin={p.icon === "working"} />{p.text}</span>)}
        <span className="muted small">{design ? `Draft saved ${clockTime(w.updated_at_ms)} · nothing is running yet` : `${s.sessions} session${s.sessions === 1 ? "" : "s"} · ${s.hosts.length} host${s.hosts.length === 1 ? "" : "s"} · ${money(s.cost)} · started ${clockTime(w.created_at_ms)}`}</span>
        <span className="sp" />
        <Button size="sm" icon="plus" kbd={design ? "A" : undefined} onClick={() => setAdding({})}>Add agent</Button>
        {design ? <><Button size="sm" onClick={saveDraft}>Save as draft</Button><Button size="sm" variant="primary" icon="send" pending={launching} disabled={!w.nodes.length} onClick={() => void launch()}>Launch</Button></>
          : w.state !== "archived" ? <Button size="sm" icon={w.state === "paused" ? "send" : "pause"} onClick={pauseResume}>{w.state === "paused" ? "Resume" : "Pause all"}</Button> : null}
      </div>
      <div className="gx">
        <Canvas view={v} design={design} lanesFor={lanesFor} sel={sel} onSelect={setSel} onConnect={onConnect} onMove={onMove} onAddOn={(host) => setAdding({ host })} />
        {sel?.kind === "node" ? <NodeInspector view={v} nodeId={sel.id} design={design} />
          : sel?.kind === "edge" ? <EdgeInspector view={v} from={sel.from} to={sel.to} design={design} />
          : design ? <DesignPanel view={v} onLaunch={() => void launch()} onSave={saveDraft} launching={launching} />
          : v.edges.find((e) => e.pending_gates.length) ? <EdgeInspector view={v} from={v.edges.find((e) => e.pending_gates.length)!.from} to={v.edges.find((e) => e.pending_gates.length)!.to} design={false} />
          : <aside className="insp"><div className="faint small">Select an agent or a connection.</div></aside>}
      </div>
      <NewSessionDialog open={!!adding} preset={adding} onClose={() => setAdding(null)} onAdd={onAdd} title={`Add agent to ${w.name}`}
        subtitle={design ? "Same form as New session. Nothing starts until you launch the workstream." : "The agent starts at once and joins this workstream."} />
    </>
  );
}
