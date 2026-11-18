import { useRef } from "react";
import type { GateRequest, PendingGate } from "../../bridge/wire";
import { agoLong } from "../../lib/format";
import { useShortcut } from "../../shortcuts/useShortcut";
import { useDriver, type SessionView } from "../../state";
import { Button, Icon, Kbd } from "../../ui";
import type { GateDensity } from "./Gate";
import { SourceCaption } from "./parts";
import { useQuestionGate } from "./useGate";

type P = PendingGate & { gate: Extract<GateRequest, { gate_type: "question" }> };
type Q = ReturnType<typeof useQuestionGate>;

function Keys({ q, focusOther }: { q: Q; focusOther: (qi: number) => void }) {
  const first = q.qs[0];
  const n = first?.options?.length ?? 0;
  for (let i = 0; i < 4; i++) {
    // eslint-disable-next-line react-hooks/rules-of-hooks
    useShortcut({
      id: `gate-digit-${i + 1}`,
      keys: [String(i + 1)],
      title: `Pick option ${i + 1} (last number = Other)`,
      group: "Answering a gate", priority: 10,
      when: () => i <= n,
      run: () => {
        if (i < n) q.toggle(0, first.options![i].label);
        else {
          q.pickOther(0);
          focusOther(0);
        }
      },
    });
  }
  useShortcut({ id: "gate-send", keys: ["enter"], title: "Send the answer", group: "Answering a gate", priority: 10, when: () => q.complete, run: q.send, allowInInput: true });
  return null;
}

/** Single choice, multi choice and free-text "Other"; options are the agent's own. */
export function QuestionGate({ view, pg, density, active, expanded, onActivate }: { view: SessionView; pg: P; density: GateDensity; active: boolean; expanded: boolean; onActivate?: () => void }) {
  const q = useQuestionGate(view, pg);
  const d = useDriver(view.driver);
  const otherRefs = useRef<Record<number, HTMLInputElement | null>>({});
  const focusOther = (qi: number) => setTimeout(() => otherRefs.current[qi]?.focus(), 0);
  const src = <SourceCaption source="agent" driver={view.driver} />;
  const send = (
    <Button size="sm" variant="primary" icon="send" disabled={!q.complete} pending={q.busy} onClick={q.send} kbd={active ? "Enter" : undefined}>
      {density === "full" ? "Send answer" : "Send"}
    </Button>
  );
  const keys = active ? <Keys q={q} focusOther={focusOther} /> : null;
  const firstQ = q.qs[0];

  const otherInput = (qi: number, cls: string) => (
    <input
      ref={(el) => (otherRefs.current[qi] = el)}
      className={cls}
      value={q.st.other[qi] ?? ""}
      onChange={(e) => q.setOther(qi, e.target.value)}
      onFocus={() => q.pickOther(qi)}
      placeholder={density === "full" ? "Type your own answer" : "type your own answer"}
      aria-label="Other answer"
      disabled={q.busy}
    />
  );

  if (density === "row" && !expanded)
    return (
      <div className="gate-inline compact" data-gate="question">
        <Icon name="needs" size="sm" className="c-needs" />
        <span className="gs">
          {firstQ.question} <span className="faint">{firstQ.options?.length ?? 0} options + Other</span>
        </span>
        {src}
        <Button size="sm" variant="warn" onClick={onActivate}>Answer</Button>
      </div>
    );

  if (density === "row")
    return (
      <div className="gate-inline" data-gate="question">
        {keys}
        {q.qs.map((qq, qi) => (
          <div key={qi} style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <div className="gq">
              <Icon name="needs" />
              <div>
                <b>{qq.question}</b>
                {qq.header ? <div className="faint" style={{ fontSize: 11.5 }}>{qq.header}{qq.multi_select ? " · choose any" : ""}</div> : null}
              </div>
            </div>
            <div className="opts" role={qq.multi_select ? "group" : "radiogroup"} aria-label={qq.question}>
              {(qq.options ?? []).map((o, i) => {
                const on = (q.st.picked[qi] ?? []).includes(o.label);
                return (
                  <button key={o.label} type="button" role={qq.multi_select ? "checkbox" : "radio"} aria-checked={on} className={`opt ${on ? "on" : ""}`} onClick={() => q.toggle(qi, o.label)} disabled={q.busy} title={o.description}>
                    <span className="rad" />
                    {o.label}
                    {qi === 0 ? <span className="k">{i + 1}</span> : null}
                  </button>
                );
              })}
              <span className={`opt ${q.st.otherOn[qi] ? "on" : ""}`} onClick={() => otherRefs.current[qi]?.focus()}>
                <span className="rad" />
                Other
                {otherInput(qi, "other")}
                {qi === 0 ? <span className="k">{(qq.options?.length ?? 0) + 1}</span> : null}
              </span>
            </div>
          </div>
        ))}
        <div className="acts">
          <span className="sp" />
          {src}
          {send}
        </div>
      </div>
    );

  return (
    <div className="gate" data-gate="question">
      {keys}
      <div className="gate-h">
        <Icon name="needs" size="sm" />
        Question from {d?.displayName ?? "the agent"}
        <span className="sp" />
        <span className="faint" style={{ fontWeight: 500 }}>asked {agoLong(pg.raised_at_ms)}</span>
      </div>
      {q.qs.map((qq, qi) => (
        <div key={qi} style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div className="q">{qq.question}</div>
          <div className="qopts" role={qq.multi_select ? "group" : "radiogroup"} aria-label={qq.question}>
            {(qq.options ?? []).map((o, i) => {
              const on = (q.st.picked[qi] ?? []).includes(o.label);
              return (
                <div key={o.label} role={qq.multi_select ? "checkbox" : "radio"} aria-checked={on} tabIndex={0} className={`qopt ${on ? "on" : ""}`} onClick={() => !q.busy && q.toggle(qi, o.label)} onKeyDown={(e) => (e.key === " " || e.key === "Enter") && e.target === e.currentTarget && (e.preventDefault(), q.toggle(qi, o.label))}>
                  <span className="rad" />
                  <div>
                    <div>{o.label}</div>
                    {o.description ? <div className="d">{o.description}</div> : null}
                  </div>
                  {qi === 0 ? <span className="k"><Kbd>{String(i + 1)}</Kbd></span> : null}
                </div>
              );
            })}
            <div className={`qopt ${q.st.otherOn[qi] ? "on" : ""}`} onClick={() => otherRefs.current[qi]?.focus()}>
              <span className="rad" />
              <div style={{ flex: 1 }}>
                <div>Other</div>
                <div className="optother" style={{ marginTop: 3 }}>{otherInput(qi, "other")}</div>
              </div>
              {qi === 0 ? <span className="k"><Kbd>{String((qq.options?.length ?? 0) + 1)}</Kbd></span> : null}
            </div>
          </div>
        </div>
      ))}
      <div className="acts">
        {send}
        <span className="sp" />
        {src}
      </div>
    </div>
  );
}
