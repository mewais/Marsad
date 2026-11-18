import { useState } from "react";
import type { GateRequest, PendingGate } from "../../bridge/types";
import { useShortcut } from "../../shortcuts/useShortcut";
import type { SessionView } from "../../state";
import { Button, Icon, Kbd } from "../../ui";
import { HANDOFF_CHOICES } from "./choices";
import type { GateDensity } from "./Gate";
import { SourceCaption } from "./parts";
import { useGateCommon } from "./useGate";

type P = PendingGate & { gate: Extract<GateRequest, { gate_type: "handoff" }> };

function Keys({ pick }: { pick: (i: number) => void }) {
  for (let i = 0; i < 3; i++) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useShortcut({ id: `gate-digit-${i + 1}`, keys: [String(i + 1)], title: `Pick handoff choice ${i + 1}`, group: "Answering a gate", priority: 10, run: () => pick(i) });
  }
  useShortcut({ id: "gate-allow", keys: ["a"], title: "Allow handoff once", group: "Answering a gate", priority: 10, run: () => pick(0) });
  useShortcut({ id: "gate-deny", keys: ["d"], title: "Deny handoff", group: "Answering a gate", priority: 10, run: () => pick(2) });
  return null;
}

/** Handoffs are Marsad's own gate (agent to agent, via Wasl): always the same three choices. */
export function HandoffGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const c = useGateCommon(view, pg);
  const [payload, setPayload] = useState(false);
  const g = pg.gate;
  const pick = (i: number) => void c.actions.decideHandoff(view.key, g.request_id, HANDOFF_CHOICES[i].id, c.localHost);
  const buttons = (keys: boolean) => (
    <div className="choices" role="group" aria-label="Handoff choices" aria-busy={c.busy || undefined}>
      {HANDOFF_CHOICES.map((o, i) => (
        <Button key={o.id} size="sm" variant={o.kind} disabled={c.busy} icon={i === 0 ? "check" : undefined} onClick={() => pick(i)}>
          {o.label}
          {keys ? <Kbd>{String(i + 1)}</Kbd> : null}
        </Button>
      ))}
    </div>
  );
  const payloadBox = payload && g.payload ? <div className="payload" data-testid="handoff-payload">{g.payload}</div> : null;
  const keysOn = active;
  const keys = active ? <Keys pick={pick} /> : null;

  if (density === "row" && !expanded)
    return (
      <div className="gate-inline compact" data-gate="handoff">
        {keys}
        <Icon name="handoff" size="sm" className="c-needs" />
        <span className="gs">Handoff from <b>{g.from_name}</b>: “{g.task}”</span>
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
          <div><b>Handoff</b> from <b>{g.from_name}</b> to <b>{g.to_name}</b><br />“{g.task}”</div>
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
        <div className="end"><span className="eyebrow">From</span><b>{g.from_name}</b></div>
        <Icon name="chevr" size="lg" className="faint" />
        <div className="end"><span className="eyebrow">To</span><b>{g.to_name}</b></div>
      </div>
      <div className="payload"><b>Task</b>{g.task}{g.payload ? <span className="muted">{g.payload}</span> : null}</div>
      <div className="acts">{buttons(keysOn)}<span className="sp" /><SourceCaption source="handoff" /></div>
    </div>
  );
}
