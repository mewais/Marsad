import { useEffect, useMemo, useRef, useState } from "react";
import type { NodePosition, WorkstreamView } from "../../bridge/wire";
import { linkView, osWord, useAppState, useHost } from "../../state";
import { Button, ConnState, Icon, hostIcon, useEscape } from "../../ui";
import { GraphNode, NODE_H, NODE_W } from "./GraphNode";
import { arrange, labelSize, layoutGraph, type Laid, type LayoutInput } from "./layout";
import { edgeLabel, POLICY } from "./model";
import { pathD } from "./route";

export type Sel = { kind: "node"; id: string } | { kind: "edge"; from: string; to: string } | null;
export const edgeKey = (from: string, to: string) => `${from}>${to}`;

function LaneLabel({ host, count, design }: { host: string; count: number; design: boolean }) {
  const h = useHost(host);
  if (!h) return <div className="lanelab"><span className="hn">{host}</span></div>;
  const up = linkView(h.link, h.authPrompt).connected;
  return (
    <>
      <div className="lanelab">
        <span className="hn"><Icon name={hostIcon(h)} size="sm" />{h.name}</span>
        <span className="hm">{[osWord(h.os), h.local ? "Local" : h.kind.kind === "ssh" ? "SSH" : "WSL"].filter(Boolean).join(" · ")}</span>
        <ConnState host={h} />
        {!design ? <span className="hm">{count} session{count === 1 ? "" : "s"}</span> : null}
      </div>
      {design && !up ? <div className="lanenote"><Icon name="refresh" size="sm" className="c-needs" />Not connected. You can place agents here once it is back.</div> : null}
    </>
  );
}

export interface CanvasProps {
  view: WorkstreamView;
  design: boolean;
  lanesFor: string[];
  sel: Sel;
  onSelect: (s: Sel) => void;
  onConnect?: (from: string, to: string) => void;
  onMove?: (node: string, pos: NodePosition) => void;
  onAddOn?: (host: string) => void;
}

/** The graph: host lanes, nodes, orthogonal edges with policy labels; design mode adds ports, drag to connect and move. */
export function Canvas({ view, design, lanesFor, sel, onSelect, onConnect, onMove, onAddOn }: CanvasProps) {
  const w = view.workstream;
  const hosts = useAppState((s) => s.hosts);
  const [laid, setLaid] = useState<Laid | null>(null);
  const [zoom, setZoom] = useState(1);
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<{ from: string; x: number; y: number } | null>(null);
  const [move, setMove] = useState<{ id: string; dx: number; dy: number; sx: number; sy: number } | null>(null);
  const nodeHost = useMemo(() => new Map(view.nodes.map((n) => [n.node, n.host])), [view.nodes]);
  const edgeViews = useMemo(() => new Map(view.edges.map((e) => [edgeKey(e.from, e.to), e])), [view.edges]);

  const input: LayoutInput = useMemo(() => ({
    lanes: lanesFor.map((host) => ({ host })),
    nodes: w.nodes.map((n) => ({ id: n.id, host: nodeHost.get(n.id) ?? n.launch?.host ?? lanesFor[0], w: design ? NODE_W + 26 : NODE_W, h: NODE_H, pos: w.layout?.[n.id] ?? null })),
    edges: w.edges.map((e) => {
      const ev = edgeViews.get(edgeKey(e.from, e.to));
      const lab = ev ? edgeLabel(ev) : null;
      return { id: edgeKey(e.from, e.to), from: e.from, to: e.to, label: labelSize(lab ? `${lab.text}${lab.tasks ? ` · ${lab.tasks} tasks` : ""}` : POLICY[e.policy].word) };
    }),
  }), [w.nodes, w.edges, w.layout, lanesFor, nodeHost, design, edgeViews]);
  const shape = JSON.stringify(input);
  useEffect(() => {
    let off = false;
    layoutGraph(input).then((l) => !off && setLaid(l), () => !off && setLaid(arrange(input, {})));
    return () => {
      off = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape]);

  useEscape(!!drag || !!move, () => (setDrag(null), setMove(null)));
  useEffect(() => {
    if (!drag && !move) return;
    const toCanvas = (e: PointerEvent) => {
      const r = canvas.current!.getBoundingClientRect();
      return { x: (e.clientX - r.left) / zoom, y: (e.clientY - r.top) / zoom };
    };
    const onMoveEv = (e: PointerEvent) => {
      const p = toCanvas(e);
      if (drag) setDrag((d) => d && { ...d, ...p });
      if (move) setMove((m) => m && { ...m, dx: p.x - m.sx, dy: p.y - m.sy });
    };
    const onUp = (e: PointerEvent) => {
      if (drag) {
        const target = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest("[data-node]")?.getAttribute("data-node");
        if (target && target !== drag.from) onConnect?.(drag.from, target);
        setDrag(null);
      }
      if (move && laid) {
        const r = laid.nodes[move.id];
        const lane = laid.lanes.find((l) => l.host === nodeHost.get(move.id));
        if (r && lane && Math.abs(move.dx) + Math.abs(move.dy) > 4) onMove?.(move.id, { x: Math.round(r.x + move.dx), y: Math.max(8, Math.round(r.y + move.dy - lane.y)) });
        setMove(null);
      }
    };
    window.addEventListener("pointermove", onMoveEv);
    window.addEventListener("pointerup", onUp);
    return () => {
      window.removeEventListener("pointermove", onMoveEv);
      window.removeEventListener("pointerup", onUp);
    };
  }, [drag, move, zoom, laid, nodeHost, onConnect, onMove]);

  const fit = () => {
    if (!laid || !wrap.current) return;
    setZoom(Math.max(0.5, Math.min(1.25, (wrap.current.clientWidth - 24) / laid.width)));
  };
  const pending = view.edges.find((e) => e.pending_gates.length);
  const spawnWait = view.pending_spawns?.[0];
  const spawnGate = spawnWait?.gate.gate_type === "spawn" ? spawnWait.gate : null;

  return (
    <div className="canvas-wrap" ref={wrap}>
      {spawnWait && spawnGate && !design ? (
        <div className="gbanner" role="status" data-testid="spawn-banner">
          <Icon name="sparkle" size="sm" className="c-needs" />
          <span><b>Spawn waiting</b> · {spawnGate.from} wants to start {spawnGate.name} on {spawnGate.host}</span>
          <Button size="sm" variant="warn" onClick={() => onSelect({ kind: "node", id: spawnWait.node })}>Review</Button>
        </div>
      ) : pending && !design ? (
        <div className="gbanner" role="status">
          <Icon name="handoff" size="sm" className="c-needs" />
          <span><b>Handoff waiting</b> · {w.nodes.find((n) => n.id === pending.from)?.name} → {w.nodes.find((n) => n.id === pending.to)?.name}</span>
          <Button size="sm" variant="warn" kbd="R" onClick={() => onSelect({ kind: "edge", from: pending.from, to: pending.to })}>Review</Button>
        </div>
      ) : null}
      {!laid ? <div className="canvas-loading faint" role="status">Laying out the graph…</div> : (
        <div className="canvas" ref={canvas} style={{ width: laid.width, height: laid.height, transform: zoom !== 1 ? `scale(${zoom})` : undefined }} data-testid="graph-canvas">
          {laid.lanes.map((l) => {
            const h = hosts[l.host];
            const up = h ? linkView(h.link, h.authPrompt).connected : false;
            const n = w.nodes.filter((x) => nodeHost.get(x.id) === l.host).length;
            return (
              <div key={l.host} className={`lanebg${design && !up ? " off" : ""}`} style={{ top: l.y, height: l.h }} data-lane={l.host}>
                <LaneLabel host={l.host} count={n} design={design} />
                {design && up && n === 0 ? <button type="button" className="slot" onClick={() => onAddOn?.(l.host)}><Icon name="plus" size="sm" />Add agent on {h?.entry.name ?? l.host}</button> : null}
              </div>
            );
          })}
          <svg className="gsvg" width={laid.width} height={laid.height} aria-hidden="true">
            <defs>
              {(["ask", "always_allow", "deny"] as const).map((p) => <marker key={p} id={`ar-${p}`} viewBox="0 0 10 10" refX="8.5" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 10 5 0 10z" fill={POLICY[p].color} /></marker>)}
            </defs>
            {w.edges.map((e) => {
              const k = edgeKey(e.from, e.to);
              const g = laid.edges[k];
              if (!g) return null;
              const on = sel?.kind === "edge" && edgeKey(sel.from, sel.to) === k;
              return <path key={k} d={pathD(g.points)} stroke={POLICY[e.policy].color} strokeWidth={on ? 3 : 1.8} strokeDasharray={e.policy === "deny" ? "5 4" : undefined} markerEnd={`url(#ar-${e.policy})`} data-edge={k} />;
            })}
            {drag && laid.nodes[drag.from] ? <path className="drawing" d={`M${laid.nodes[drag.from].x + laid.nodes[drag.from].w} ${laid.nodes[drag.from].y + laid.nodes[drag.from].h / 2} L${drag.x} ${drag.y}`} /> : null}
          </svg>
          {w.nodes.map((n) => {
            const r = laid.nodes[n.id];
            if (!r) return null;
            const off = move?.id === n.id ? { x: move.dx, y: move.dy } : { x: 0, y: 0 };
            return (
              <GraphNode key={n.id} view={view} node={n} rect={{ ...r, x: r.x + off.x, y: r.y + off.y }} design={design} selected={sel?.kind === "node" && sel.id === n.id} connecting={!!drag}
                onSelect={() => onSelect({ kind: "node", id: n.id })}
                onPortDown={(e) => { e.preventDefault(); e.stopPropagation(); setDrag({ from: n.id, x: r.x + r.w, y: r.y + r.h / 2 }); }}
                onBodyDown={design ? (e) => { const c = canvas.current!.getBoundingClientRect(); setMove({ id: n.id, dx: 0, dy: 0, sx: (e.clientX - c.left) / zoom, sy: (e.clientY - c.top) / zoom }); } : undefined} />
            );
          })}
          {w.edges.map((e) => {
            const k = edgeKey(e.from, e.to);
            const g = laid.edges[k];
            const ev = edgeViews.get(k);
            if (!g || !ev) return null;
            const lab = edgeLabel(ev);
            const on = sel?.kind === "edge" && edgeKey(sel.from, sel.to) === k;
            return (
              <button key={k} type="button" className={`el ${lab.pending ? "pending" : POLICY[e.policy].cls}${on && !lab.pending ? " sel" : ""}`} style={{ left: g.label.x, top: g.label.y }} data-edge={k}
                aria-label={`Connection ${w.nodes.find((x) => x.id === e.from)?.name} to ${w.nodes.find((x) => x.id === e.to)?.name}: ${lab.text}`} onClick={() => onSelect({ kind: "edge", from: e.from, to: e.to })}>
                <Icon name={lab.pending ? "needs" : POLICY[e.policy].icon} size="xs" />{lab.text}
                {lab.tasks ? <span className="tk">· {lab.tasks} task{lab.tasks === 1 ? "" : "s"}</span> : null}
              </button>
            );
          })}
          {drag ? <div className="draghint" style={{ left: drag.x + 16, top: drag.y + 16 }}><Icon name="handoff" size="sm" /><span><b>Drawing a connection.</b> Release on an agent to connect them. Esc cancels.</span></div> : null}
        </div>
      )}
      <div className="glegend" aria-label="Legend">
        {design ? <span><Icon name="handoff" size="xs" className="c-accent" />Drag from a dot on an agent to connect it</span> : null}
        {(["ask", "always_allow", "deny"] as const).map((p) => <span key={p}><svg width="22" height="8" aria-hidden="true"><path d="M0 4h22" stroke={POLICY[p].color} strokeWidth="2" strokeDasharray={p === "deny" ? "5 4" : undefined} /></svg>{POLICY[p].word}</span>)}
      </div>
      <div className="gctl">
        <Button size="sm" icon="minus" aria-label="Zoom out" onClick={() => setZoom((z) => Math.max(0.5, +(z - 0.1).toFixed(2)))} />
        <span className="btn sm tnum zoomv" aria-live="polite">{Math.round(zoom * 100)}%</span>
        <Button size="sm" icon="plus" aria-label="Zoom in" onClick={() => setZoom((z) => Math.min(1.5, +(z + 0.1).toFixed(2)))} />
        <Button size="sm" onClick={fit}>Fit</Button>
      </div>
    </div>
  );
}
