import { useRef } from "react";
import type { GateRequest, PendingGate } from "../../bridge/wire";
import { useShortcut } from "../../shortcuts/useShortcut";
import type { SessionView } from "../../state";
import { Icon } from "../../ui";
import type { GateDensity } from "./Gate";
import { ChoiceButtons, ReasonBox, SourceCaption } from "./parts";
import { usePermissionGate } from "./useGate";

type SpawnRequest = Extract<GateRequest, { gate_type: "spawn" }>;
type P = PendingGate & { gate: SpawnRequest };

/** "Claude Code · sonnet · plan mode": what would run. */
export function spawnWhat(g: SpawnRequest): string {
  return [g.driver, g.model, g.mode ? `${g.mode} mode` : null].filter(Boolean).join(" · ");
}

/** One line: who wants to start whom, where. */
export function spawnLine(g: SpawnRequest): string {
  return `${g.from} wants to start ${g.name} on ${g.host}`;
}

/** The workstream note: which workstream it joins, or that approving creates one. */
export function spawnWorkstreamNote(g: SpawnRequest): string {
  return g.new_workstream
    ? `${g.from} is in no workstream: approving creates the workstream “${g.workstream}” with both agents.`
    : `Joins workstream ${g.workstream}, connected from ${g.from}.`;
}

function Keys({ g }: { g: ReturnType<typeof usePermissionGate> }) {
  for (let i = 0; i < 4; i++) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useShortcut({ id: `gate-digit-${i + 1}`, keys: [String(i + 1)], title: `Pick spawn choice ${i + 1}`, group: "Answering a gate", priority: 10, when: () => i < g.set.choices.length, run: () => g.choose(g.set.choices[i]) });
  }
  useShortcut({ id: "gate-allow", keys: ["a"], title: "Allow this spawn once", group: "Answering a gate", priority: 10, when: () => !!g.first("allow"), run: () => g.choose(g.first("allow")!) });
  useShortcut({ id: "gate-deny", keys: ["d"], title: "Deny this spawn", group: "Answering a gate", priority: 10, when: () => !!g.first("deny"), run: () => g.choose(g.first("deny")!) });
  return null;
}

/** An agent asks to start a worker agent: Marsad's own gate with its fixed choices. */
export function SpawnGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const sg = usePermissionGate(view, pg);
  const g = pg.gate;
  const reasonRef = useRef<HTMLInputElement>(null);
  const deny = sg.set.choices.find((c) => c.text !== "none");
  const buttons = (keys: boolean) => <ChoiceButtons choices={sg.set.choices} busy={sg.busy} blocked={sg.blocked} onChoose={(c) => sg.choose(c)} keys={keys} />;
  const reason = deny ? <ReasonBox who={g.from} value={sg.reason} onChange={sg.setReason} inputRef={reasonRef} onSubmit={() => sg.choose(deny)} label={deny.label.map((p) => p.text).join("")} /> : null;
  const keys = active ? <Keys g={sg} /> : null;
  const facts = (
    <dl className="spawnfacts" data-testid="spawn-facts">
      <dt>Agent</dt><dd><b>{g.name}</b> · {spawnWhat(g)}</dd>
      <dt>Where</dt><dd>{g.host} · <span className="mono">{g.cwd}</span></dd>
      <dt>Why</dt><dd>{g.role}</dd>
      {g.instruction ? <><dt>First task</dt><dd>“{g.instruction}”</dd></> : null}
    </dl>
  );

  if (density === "row" && !expanded)
    return (
      <div className="gate-inline compact" data-gate="spawn">
        {keys}
        <Icon name="sparkle" size="sm" className="c-needs" />
        <span className="gs">{spawnLine(g)}: “{g.role}”</span>
        <SourceCaption source="spawn" />
        {buttons(false)}
        <button type="button" className="iconbtn" aria-label="Review the spawn request" title="Review the spawn request" onClick={onActivate}><Icon name="chevr" size="sm" /></button>
      </div>
    );
  if (density === "row")
    return (
      <div className="gate-inline" data-gate="spawn">
        {keys}
        <div className="gq">
          <Icon name="sparkle" />
          <div><b>Spawn a worker</b> · {spawnLine(g)}</div>
        </div>
        {facts}
        <div className={`spawnnote${g.new_workstream ? " new" : ""}`}><Icon name="graph" size="xs" />{spawnWorkstreamNote(g)}</div>
        <div className="acts">
          {buttons(active)}
          <span className="sp" />
          <SourceCaption source="spawn" />
        </div>
        {reason}
      </div>
    );
  return (
    <div className="gate" data-gate="spawn">
      {keys}
      <div className="gate-h"><Icon name="sparkle" size="sm" />Spawn request waiting for you</div>
      <div className="q"><b>{g.from}</b> wants to start a new agent, <b>{g.name}</b>.</div>
      {facts}
      <div className={`spawnnote${g.new_workstream ? " new" : ""}`}><Icon name="graph" size="xs" />{spawnWorkstreamNote(g)}</div>
      {sg.set.choices.some((c) => c.description) ? (
        <ul className="choice-notes faint fs115">
          {sg.set.choices.filter((c) => c.description).map((c) => <li key={c.id}><b>{c.label.map((p) => p.text).join("")}</b>: {c.description}</li>)}
        </ul>
      ) : null}
      <div className="acts">{buttons(active)}<span className="sp" /><SourceCaption source="spawn" /></div>
      {reason}
    </div>
  );
}
