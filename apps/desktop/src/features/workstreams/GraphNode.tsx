import { memo } from "react";
import type { WorkstreamNode, WorkstreamView } from "../../bridge/wire";
import { basename, money } from "../../lib/format";
import { useAppState, useStaleHosts } from "../../state";
import { DriverBadge, Icon, STATUS, makeDriver } from "../../ui";
import type { Rect } from "./route";
import { nodeName, nodeStatus, taskWords } from "./model";

export const NODE_W = 230;
export const NODE_H = 112;

export interface GraphNodeProps {
  view: WorkstreamView;
  node: WorkstreamNode;
  rect: Rect;
  design: boolean;
  selected: boolean;
  connecting: boolean;
  onSelect: () => void;
  onPortDown: (e: React.PointerEvent) => void;
  onBodyDown?: (e: React.PointerEvent) => void;
}

/** One agent: driver, name, live status in words, cost, and the task it is working on. */
export const GraphNode = memo(function GraphNode({ view, node, rect, design, selected, connecting, onSelect, onPortDown, onBodyDown }: GraphNodeProps) {
  const nv = view.nodes.find((n) => n.node === node.id);
  const snap = nv?.session ?? null;
  const driverId = snap?.driver ?? node.launch?.driver ?? "";
  const drv = useAppState((s) => s.drivers[driverId]) ?? makeDriver(driverId || "?");
  const staleHosts = useStaleHosts();
  const st = nodeStatus(nv, snap, !!nv && staleHosts.has(nv.host));
  const incoming = view.edges.filter((e) => e.to === node.id).flatMap((e) => e.in_flight);
  const outgoing = view.edges.filter((e) => e.from === node.id).flatMap((e) => e.in_flight);
  const task = incoming[0];
  const meta = STATUS[st.status as keyof typeof STATUS];
  const cls = `gn${node.spawned_by ? " spawned" : ""}${design ? " design" : ""}${selected ? " sel" : ""}${outgoing.length && !incoming.length ? " orch" : ""}${connecting ? " target" : ""}`;
  return (
    <div className={cls} style={{ left: rect.x, top: rect.y, width: rect.w }} data-node={node.id} role="button" tabIndex={0} aria-pressed={selected}
      aria-label={`${node.name}${node.spawned_by ? ` (spawned by ${nodeName(view, node.spawned_by)})` : ""}: ${st.word}`} onClick={onSelect} onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onSelect())}
      onPointerDown={(e) => { if (e.button === 0 && onBodyDown && !(e.target as HTMLElement).closest(".port")) onBodyDown(e); }}>
      <div className="t"><DriverBadge driver={drv} compact /><span>{node.name}</span>{node.spawned_by ? <span className="spawnedby" data-testid="spawned-by" title={`Spawned by ${nodeName(view, node.spawned_by)}${node.role ? `: ${node.role}` : ""}`}><Icon name="sparkle" size="xs" />by {nodeName(view, node.spawned_by)}</span> : null}</div>
      {design ? (
        <>
          <div className="row">
            <span className="tag">{node.session ? <><Icon name="refresh" size="xs" />Pulled in · keeps history</> : "New"}</span>
            {node.launch?.model ? <span className="faint ell">{node.launch.model}</span> : null}
            <span className="sp" /><span className="wasl" title="The Wasl connector will be added"><Icon name="lock" size="xs" />Wasl</span>
          </div>
          <div className="meta">
            <span className="mono ell">{node.launch?.cwd ?? snap?.cwd ?? ""}</span>
            <span className="ell">{node.launch?.first_prompt ? `First task: “${node.launch.first_prompt}”` : node.session ? `${basename(snap?.cwd ?? "")} · ${st.word}` : "No first task. Waits for a handoff."}</span>
          </div>
        </>
      ) : (
        <>
          <div className="row">
            {meta ? <span className={`st ${st.status}${st.stale ? " stale" : ""}`}><Icon name={meta.icon} size="sm" spin={st.status === "working" && !st.stale} />{st.word}</span> : <span className="st idle"><Icon name={st.status === "launching" ? "working" : "idle"} size="sm" spin={st.status === "launching"} />{st.word}</span>}
            <span className="sp" />
            <span className="tnum">{money(snap?.usage.cost_usd ?? 0)}</span>
          </div>
          {task ? (
            <div className="task"><span className="fg ell">{task.task.instruction}</span><span className="ell">{taskWords(task)}</span></div>
          ) : outgoing.length ? (
            <div className="task"><span className="ell"><Icon name="handoff" size="xs" /> Delegating {outgoing.length} task{outgoing.length === 1 ? "" : "s"}</span></div>
          ) : node.spawned_by && node.role ? (
            <div className="task"><span className="ell">{node.role}</span></div>
          ) : null}
        </>
      )}
      {design ? <span className="port out" role="button" tabIndex={-1} aria-label={`Connect ${node.name} to another agent`} title="Drag to another agent to connect" onPointerDown={onPortDown} /> : null}
    </div>
  );
});
