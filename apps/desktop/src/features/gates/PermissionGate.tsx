import { useEffect, useId, useRef, useState } from "react";
import type { GateRequest, PendingGate } from "../../bridge/wire";
import { agoLong } from "../../lib/format";
import { useShortcut } from "../../shortcuts/useShortcut";
import { useDriver, useHost, type SessionView } from "../../state";
import { Button, Icon } from "../../ui";
import { ChoiceButtons, ChoiceEffects, ReasonBox, SourceCaption } from "./parts";
import type { GateDensity } from "./Gate";
import { usePermissionGate } from "./useGate";
import { permissionKind, permissionLine, permissionTitle } from "../../lib/gatewords";

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

const title = (g: P["gate"]) => (g.gate_type === "plan" ? "Plan waiting for approval" : permissionTitle(g));
/** The gate's one line: a plan's summary, "Run <command>", a question as plain text, "<tool>: <what>". */
function Line({ g }: { g: P["gate"] }) {
  if (g.gate_type === "plan") return <>Plan: <code>{g.summary}</code></>;
  const l = permissionLine(g);
  return <>{l.prefix}{l.code ? <code>{l.text}</code> : <span className="gq-text">{l.text}</span>}</>;
}

export function PermissionGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const g = usePermissionGate(view, pg);
  const d = useDriver(view.driver);
  const host = useHost(view.host);
  const effectsPrefix = useId();
  const [details, setDetails] = useState(false);
  const reasonRef = useRef<HTMLInputElement>(null);
  const focusReason = () => setTimeout(() => reasonRef.current?.focus(), 0);
  const gate = pg.gate;
  const isPlan = gate.gate_type === "plan";
  const isQuestion = gate.gate_type === "permission" && permissionKind(gate) === "question";
  const textChoice = g.set.choices.find((c) => c.text !== "none");
  const needsReason = !!textChoice;
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
          <Icon name={isPlan ? "list" : isQuestion ? "needs" : "shield"} size="sm" className="c-needs" />
          <span className="gs"><Line g={gate} /></span>
          {src}
          <ChoiceButtons choices={g.set.choices} busy={g.busy} blocked={g.blocked} onChoose={choose} effectsPrefix={effectsPrefix} />
          <button type="button" className="iconbtn" aria-label="Review details" title="Review details" onClick={onActivate}>
            <Icon name="chevr" size="sm" />
          </button>
          <ChoiceEffects choices={g.set.choices} prefix={effectsPrefix} />
        </div>
      ) : density === "row" ? (
        <div className="gate-inline" data-gate="permission">
          <div className="gq">
            <Icon name={isPlan ? "list" : isQuestion ? "needs" : "shield"} />
            <div>
              <b>{title(gate)}</b> on {host?.name ?? view.host}
              <br />
              {gate.gate_type === "plan" ? <code>{gate.summary}</code> : isQuestion ? <span className="gq-text">{gate.command}</span> : <code>{gate.command}</code>}
            </div>
          </div>
          <div className="acts">
            <ChoiceButtons choices={g.set.choices} busy={g.busy} blocked={g.blocked} onChoose={choose} keys={keys} effectsPrefix={effectsPrefix} />
            <span className="sp" />
            {src}
            <Button size="sm" variant="ghost" onClick={() => setDetails((v) => !v)} aria-expanded={details}>
              Review details <Icon name="chevr" size="xs" />
            </Button>
          </div>
          <ChoiceEffects choices={g.set.choices} prefix={effectsPrefix} />
          {detailBox}
          {showReason ? <ReasonBox who={d?.displayName ?? "the agent"} value={g.reason} onChange={g.setReason} inputRef={reasonRef} onSubmit={() => textChoice && g.choose(textChoice)} label={textChoice ? textChoice.label.map((p) => p.text).join("") : undefined} /> : null}
        </div>
      ) : (
        <div className="gate" data-gate="permission">
          <div className="gate-h">
            <Icon name={isPlan ? "list" : isQuestion ? "needs" : "shield"} size="sm" />
            {isPlan ? "Plan waiting for approval" : isQuestion ? "Question" : `Permission needed · ${gate.tool_name}`}
            <span className="sp" />
            <span className="faint fw5">asked {agoLong(pg.raised_at_ms)}</span>
          </div>
          {isPlan ? <div className="q prewrap">{gate.summary}</div> : isQuestion ? <div className="q prewrap">{gate.command}</div> : <div className="cmd">{gate.command}</div>}
          {g.set.choices.some((c) => c.description && c.effects.length === 0) ? (
            <ul className="choice-notes faint fs115">
              {g.set.choices.filter((c) => c.description && c.effects.length === 0).map((c) => (
                <li key={c.id}><b>{c.label.map((p) => p.text).join("")}</b>: {c.description}</li>
              ))}
            </ul>
          ) : null}
          {!isPlan && !isQuestion && gate.explanation ? (
            <div className="risk">
              <Icon name="warn" size="sm" className="c-needs" />
              <span>{gate.explanation}</span>
            </div>
          ) : null}
          <div className="acts">
            <ChoiceButtons choices={g.set.choices} busy={g.busy} blocked={g.blocked} onChoose={choose} keys={keys} effectsPrefix={effectsPrefix} />
            <span className="sp" />
            {src}
          </div>
          <ChoiceEffects choices={g.set.choices} prefix={effectsPrefix} />
          {showReason ? <ReasonBox who={d?.displayName ?? "the agent"} value={g.reason} onChange={g.setReason} inputRef={reasonRef} onSubmit={() => textChoice && g.choose(textChoice)} label={textChoice ? textChoice.label.map((p) => p.text).join("") : undefined} /> : null}
        </div>
      )}
    </>
  );
  return body;
}
