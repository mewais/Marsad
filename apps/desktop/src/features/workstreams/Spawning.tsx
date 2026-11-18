import { useEffect, useState } from "react";
import type { SpawnPolicy, SpawnSettings, WorkstreamView } from "../../bridge/wire";
import { sessionKey } from "../../bridge/wire";
import { useActions, useApi, useAppState, useHosts, usePending, useSessionView } from "../../state";
import { Icon } from "../../ui";
import { Gate } from "../gates/Gate";

/** Spawn policy words (the workstream's rule for agents starting workers). */
export const SPAWN_POLICY: Record<SpawnPolicy, { word: string; desc: string; icon: "shield" | "check" | "ban"; cls: string }> = {
  ask: { word: "Ask each time", desc: "Every spawn request stops for your approval.", icon: "shield", cls: "c-needs" },
  always_allow: { word: "Always allow", desc: "Agents start workers within the limits below without asking.", icon: "check", cls: "c-done" },
  never: { word: "Never", desc: "Agents are told spawning is turned off here.", icon: "ban", cls: "c-failed" },
};

export const DEFAULT_SPAWN: SpawnSettings = { policy: "ask", handoffs_always_allow: false, max_agents: 8, max_depth: 2, allowed_hosts: [], allowed_drivers: [] };
export const spawnOf = (v: WorkstreamView): SpawnSettings => ({ ...DEFAULT_SPAWN, ...(v.workstream.spawn ?? {}) });
export const spawnedCount = (v: WorkstreamView): number => v.workstream.nodes.filter((n) => n.spawned_by).length;

/** The first spawn gate waiting in the workstream, with the asking session's view. */
export function PendingSpawn({ view, node }: { view: WorkstreamView; node?: string }) {
  const g = view.pending_spawns?.find((p) => !node || p.node === node);
  const sender = useSessionView(g ? sessionKey(g.host, g.session) : "");
  if (!g || !sender) return null;
  return (
    <div className="sec" data-testid="spawn-gate">
      <Gate view={sender} pg={{ gate: g.gate, raised_at_ms: g.raised_at_ms, cursor: 0 }} density="full" active />
    </div>
  );
}

/** The workstream's "Spawning" settings: policy, whether always-allow covers handoffs, limits, allowed hosts and drivers. */
export function SpawnSection({ view }: { view: WorkstreamView }) {
  const api = useApi();
  const actions = useActions();
  const hosts = useHosts();
  const drivers = useAppState((s) => s.drivers);
  const w = view.workstream;
  const st = spawnOf(view);
  const pk = `spawn:${w.id}`;
  const busy = !!usePending(pk);
  const [maxAgents, setMaxAgents] = useState(String(st.max_agents));
  const [maxDepth, setMaxDepth] = useState(String(st.max_depth));
  useEffect(() => setMaxAgents(String(st.max_agents)), [st.max_agents]);
  useEffect(() => setMaxDepth(String(st.max_depth)), [st.max_depth]);
  const save = (next: Partial<SpawnSettings>) => void actions.run(pk, "Could not save the spawning rules", () => api.ws.update({ ...w, spawn: { ...st, ...next } }, w.revision));
  const num = (s: string, min: number) => Math.max(min, Math.min(99, Math.floor(Number(s) || 0)));
  const toggle = (list: string[] | undefined, id: string) => (list ?? []).includes(id) ? (list ?? []).filter((x) => x !== id) : [...(list ?? []), id];
  const spawned = spawnedCount(view);
  return (
    <div className="sec spawnsec" data-testid="spawn-settings">
      <div className="eyebrow">Spawning · {spawned} spawned agent{spawned === 1 ? "" : "s"}</div>
      <div className="faint small">Agents here may start worker agents of their own (spawn_agent). They join this graph, connected from the agent that started them.</div>
      <div className="pol" role="radiogroup" aria-label="Spawn policy">
        {(Object.keys(SPAWN_POLICY) as SpawnPolicy[]).map((p) => (
          <button key={p} type="button" role="radio" aria-checked={p === st.policy} disabled={busy} className={`po${p === st.policy ? " on" : ""}`} onClick={() => p !== st.policy && save({ policy: p, handoffs_always_allow: p === "always_allow" ? st.handoffs_always_allow : false })}>
            <Icon name={SPAWN_POLICY[p].icon} size="sm" className={SPAWN_POLICY[p].cls} /><span><b>{SPAWN_POLICY[p].word}</b><span className="d">{SPAWN_POLICY[p].desc}</span></span>
          </button>
        ))}
      </div>
      <label className={`small chkl${st.policy !== "always_allow" ? " faint" : ""}`}>
        <input type="checkbox" checked={st.handoffs_always_allow} disabled={busy || st.policy !== "always_allow"} onChange={(e) => save({ handoffs_always_allow: e.target.checked })} />
        Always allow also covers handoffs to agents spawned from now on (otherwise their connections ask)
      </label>
      <div className="spawnlimits">
        <label className="small">Max spawned agents
          <input className="tf sm tnum" type="number" min={0} max={99} value={maxAgents} disabled={busy} aria-label="Max spawned agents" onChange={(e) => setMaxAgents(e.target.value)} onBlur={() => num(maxAgents, 0) !== st.max_agents && save({ max_agents: num(maxAgents, 0) })} />
        </label>
        <label className="small">Max depth
          <input className="tf sm tnum" type="number" min={1} max={99} value={maxDepth} disabled={busy} aria-label="Max spawn depth" onChange={(e) => setMaxDepth(e.target.value)} onBlur={() => num(maxDepth, 1) !== st.max_depth && save({ max_depth: num(maxDepth, 1) })} />
        </label>
      </div>
      <div className="small">Hosts besides the asking agent's own</div>
      <div className="chips" role="group" aria-label="Allowed hosts">
        {hosts.map((h) => {
          const on = (st.allowed_hosts ?? []).includes(h.id);
          return <button key={h.id} type="button" className={`chip${on ? " on" : ""}`} aria-pressed={on} disabled={busy} onClick={() => save({ allowed_hosts: toggle(st.allowed_hosts, h.id) })}>{h.name}</button>;
        })}
      </div>
      <div className="small">Agent CLIs {(st.allowed_drivers ?? []).length ? "" : <span className="faint">(none picked: any installed one)</span>}</div>
      <div className="chips" role="group" aria-label="Allowed agent CLIs">
        {Object.values(drivers).map((d) => {
          const on = (st.allowed_drivers ?? []).includes(d.id);
          return <button key={d.id} type="button" className={`chip${on ? " on" : ""}`} aria-pressed={on} disabled={busy} onClick={() => save({ allowed_drivers: toggle(st.allowed_drivers, d.id) })}>{d.displayName}</button>;
        })}
      </div>
    </div>
  );
}
