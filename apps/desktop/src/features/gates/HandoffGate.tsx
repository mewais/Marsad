import { useState } from "react";
import type { GateRequest, PendingGate } from "../../bridge/wire";
import { useShortcut } from "../../shortcuts/useShortcut";
import type { SessionView } from "../../state";
import { Button, Icon } from "../../ui";
import type { GateDensity } from "./Gate";
import { ChoiceButtons, SourceCaption } from "./parts";
import { usePermissionGate } from "./useGate";

/** The handoff's structured context, shown as the agent sent it (keys and values, one per line). */
export function detailsText(d: unknown): string {
  if (d == null) return "";
  if (typeof d !== "object") return String(d);
  return Object.entries(d as Record<string, unknown>).map(([k, v]) => `${k}: ${Array.isArray(v) ? v.join(", ") : typeof v === "object" ? JSON.stringify(v) : String(v)}`).join("\n");
}

/**
 * The parts of a handoff's context a person reads: the hop count and the deadline as a time;
 * the agents' ids and the workstream id (shown elsewhere by name) stay in the raw payload.
 */
export function detailsForPeople(d: unknown): string {
  if (d == null || typeof d !== "object") return "";
  const o = d as Record<string, unknown>;
  const out: string[] = [];
  if (typeof o.hop === "number") out.push(`Hop ${o.hop}`);
  if (typeof o.deadline_ms === "number" && o.deadline_ms > 0) out.push(`Deadline ${new Date(o.deadline_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}`);
  return out.join(" · ");
}

type P = PendingGate & { gate: Extract<GateRequest, { gate_type: "handoff" }> };

function Keys({ g }: { g: ReturnType<typeof usePermissionGate> }) {
  for (let i = 0; i < 3; i++) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useShortcut({ id: `gate-digit-${i + 1}`, keys: [String(i + 1)], title: `Pick handoff choice ${i + 1}`, group: "Answering a gate", priority: 10, when: () => i < g.set.choices.length, run: () => g.choose(g.set.choices[i]) });
  }
  useShortcut({ id: "gate-allow", keys: ["a"], title: "Allow handoff once", group: "Answering a gate", priority: 10, when: () => !!g.first("allow"), run: () => g.choose(g.first("allow")!) });
  useShortcut({ id: "gate-deny", keys: ["d"], title: "Deny handoff", group: "Answering a gate", priority: 10, when: () => !!g.first("deny"), run: () => g.choose(g.first("deny")!) });
  return null;
}

/** Handoffs are Marsad's own gate (agent to agent, via Wasl), with the choices Wasl offers. */
export function HandoffGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const hg = usePermissionGate(view, pg);
  const [payload, setPayload] = useState(false);
  const g = pg.gate;
  const buttons = (keys: boolean) => <ChoiceButtons choices={hg.set.choices} busy={hg.busy} onChoose={(c) => hg.choose(c)} keys={keys} />;
  const details = detailsText(g.details);
  const payloadBox = payload ? <div className="payload" data-testid="handoff-payload"><span><b>Mode</b> {g.mode}</span>{details ? <span className="mono">{details}</span> : null}</div> : null;
  const keysOn = active;
  const keys = active ? <Keys g={hg} /> : null;

  if (density === "row" && !expanded)
    return (
      <div className="gate-inline compact" data-gate="handoff">
        {keys}
        <Icon name="handoff" size="sm" className="c-needs" />
        <span className="gs">Handoff from <b>{g.from}</b>: “{g.instruction}”</span>
        <SourceCaption source="handoff" />
        {buttons(false)}
        <button type="button" className="iconbtn" aria-label="Review payload" title="Review payload" onClick={onActivate}><Icon name="chevr" size="sm" /></button>
      </div>
    );
  if (density === "row")
    return (
      <div className="gate-inline" data-gate="handoff">
        {keys}
        <div className="gq">
          <Icon name="handoff" />
          <div><b>Handoff</b> from <b>{g.from}</b> to <b>{g.to}</b><br />“{g.instruction}”</div>
        </div>
        <div className="acts">
          {buttons(keysOn)}
          <span className="sp" />
          <SourceCaption source="handoff" />
          <Button size="sm" variant="ghost" onClick={() => setPayload((v) => !v)} aria-expanded={payload}>Review payload <Icon name="chevr" size="xs" /></Button>
        </div>
        {payloadBox}
      </div>
    );
  return (
    <div className="gate" data-gate="handoff">
      {keys}
      <div className="gate-h"><Icon name="handoff" size="sm" />Handoff waiting for you</div>
      <div className="hop">
        <div className="end"><span className="eyebrow">From</span><b>{g.from}</b></div>
        <Icon name="chevr" size="lg" className="faint" />
        <div className="end"><span className="eyebrow">To</span><b>{g.to}</b></div>
      </div>
      <div className="payload"><b>Task</b>{g.instruction}<span className="muted">Mode: {g.mode}{detailsForPeople(g.details) ? ` · ${detailsForPeople(g.details)}` : ""}</span></div>
      <div className="acts">{buttons(keysOn)}<span className="sp" /><SourceCaption source="handoff" /></div>
    </div>
  );
}
