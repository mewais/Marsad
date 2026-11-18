import { useEffect, useRef, useState } from "react";
import type { GateRequest, PendingGate } from "../../bridge/wire";
import { agoLong } from "../../lib/format";
import { useShortcut } from "../../shortcuts/useShortcut";
import { useDriver, type SessionView } from "../../state";
import { Button, Icon } from "../../ui";
import { ChoiceButtons, ReasonBox, SourceCaption } from "./parts";
import type { GateDensity } from "./Gate";
import { usePermissionGate } from "./useGate";

type P = PendingGate & { gate: Extract<GateRequest, { gate_type: "permission" | "plan" }> };

function PermissionKeys({ g }: { g: ReturnType<typeof usePermissionGate> }) {
  // Fixed number of hooks; digits beyond the choice count are skipped by `when`.
  for (let i = 0; i < 4; i++) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useShortcut({ id: `gate-digit-${i + 1}`, keys: [String(i + 1)], title: `Pick choice ${i + 1}`, group: "Answering a gate", priority: 10, when: () => i < g.set.choices.length, run: () => g.choose(g.set.choices[i]) });
  }
  useShortcut({ id: "gate-allow", keys: ["a"], title: "Allow (the agent's first allow choice)", group: "Answering a gate", priority: 10, when: () => !!g.first("allow"), run: () => g.choose(g.first("allow")!) });
  useShortcut({ id: "gate-deny", keys: ["d"], title: "Deny", group: "Answering a gate", priority: 10, when: () => !!g.first("deny"), run: () => g.choose(g.first("deny")!) });
  return null;
}

const title = (g: P["gate"]) => (g.gate_type === "plan" ? "Plan waiting for approval" : `Run ${g.tool_name === "Bash" || g.tool_name === "shell" ? "a shell command" : `${g.tool_name}`}`);
const summary = (g: P["gate"]) => (g.gate_type === "plan" ? g.summary : g.command);

export function PermissionGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const g = usePermissionGate(view, pg);
  const d = useDriver(view.driver);
  const [details, setDetails] = useState(false);
  const reasonRef = useRef<HTMLInputElement>(null);
  const focusReason = () => setTimeout(() => reasonRef.current?.focus(), 0);
  const gate = pg.gate;
  const isPlan = gate.gate_type === "plan";
  const needsReason = g.set.choices.some((c) => c.asksReason);
  const showReason = needsReason && (g.armed || g.reason !== "" || density === "full" || (density === "row" && expanded));
  const choose = (c: Parameters<typeof g.choose>[0]) => g.choose(c, focusReason);
  useEffect(() => {
    if (!active) setDetails(false);
  }, [active]);
  const keys = active;
  const src = <SourceCaption source={g.set.source} driver={view.driver} />;
  const detailBox = details && gate.gate_type === "permission" ? (
    <div className="code-inline code-block" data-testid="gate-details">
      {gate.cwd ? `in ${gate.cwd}\n` : ""}{gate.explanation ?? ""}
      {gate.raw_input ? `\n${JSON.stringify(gate.raw_input)}` : ""}
    </div>
  ) : null;

  const body = (
    <>
      {active ? <PermissionKeys g={g} /> : null}
      {density === "row" && !expanded ? (
        <div className="gate-inline compact" data-gate="permission">
          <Icon name={isPlan ? "list" : "shield"} size="sm" className="c-needs" />
          <span className="gs">{isPlan ? "Plan: " : "Run "}<code>{summary(gate)}</code></span>
          {src}
          <ChoiceButtons choices={g.set.choices} busy={g.busy} onChoose={choose} />
          <button type="button" className="iconbtn" aria-label="Review details" title="Review details" onClick={onActivate}>
            <Icon name="chevr" size="sm" />
          </button>
        </div>
      ) : density === "row" ? (
        <div className="gate-inline" data-gate="permission">
          <div className="gq">
            <Icon name={isPlan ? "list" : "shield"} />
            <div>
              <b>{title(gate)}</b> on {view.host}
              <br />
              <code>{summary(gate)}</code>
            </div>
          </div>
          <div className="acts">
            <ChoiceButtons choices={g.set.choices} busy={g.busy} onChoose={choose} keys={keys} />
            <span className="sp" />
            {src}
            <Button size="sm" variant="ghost" onClick={() => setDetails((v) => !v)} aria-expanded={details}>
              Review details <Icon name="chevr" size="xs" />
            </Button>
          </div>
          {detailBox}
          {showReason ? <ReasonBox who={d?.displayName ?? "the agent"} value={g.reason} onChange={g.setReason} inputRef={reasonRef} onSubmit={() => g.choose(g.first("deny")!)} /> : null}
        </div>
      ) : (
        <div className="gate" data-gate="permission">
          <div className="gate-h">
            <Icon name={isPlan ? "list" : "shield"} size="sm" />
            {isPlan ? "Plan waiting for approval" : `Permission needed · ${gate.tool_name}`}
            <span className="sp" />
            <span className="faint fw5">asked {agoLong(pg.raised_at_ms)}</span>
          </div>
          {isPlan ? <div className="q prewrap">{gate.summary}</div> : <div className="cmd">{gate.command}</div>}
          {!isPlan && gate.explanation ? (
            <div className="risk">
              <Icon name="warn" size="sm" className="c-needs" />
              <span>{gate.explanation}</span>
            </div>
          ) : null}
          <div className="acts">
            <ChoiceButtons choices={g.set.choices} busy={g.busy} onChoose={choose} keys={keys} />
            <span className="sp" />
            {src}
          </div>
          {showReason ? <ReasonBox who={d?.displayName ?? "the agent"} value={g.reason} onChange={g.setReason} inputRef={reasonRef} onSubmit={() => g.choose(g.first("deny")!)} /> : null}
        </div>
      )}
    </>
  );
  return body;
}
