import { CopyButton, Icon } from "../../ui";
import { stepTime, type PromptRow } from "./prompts";
import { sizeWords } from "./words";

/** One prompt and the path it took: each step with when it was seen (relative to the first) and who saw it; steps not observed are hollow. */
export function PromptPath({ row }: { row: PromptRow }) {
  const what = [row.kind === "slash" ? "Slash command" : "Prompt", `${row.chars} characters`, row.attachments ? `${row.attachments} attachment${row.attachments === 1 ? "" : "s"} (${sizeWords(row.bytes)})` : ""].filter(Boolean).join(" · ");
  // What the observers recorded beyond the step itself (the outcome of a request, a reason, an upload), one line each.
  const notes = [...row.steps.filter((s) => s.detail && s.state !== "not_seen").map((s) => `${s.label}: ${s.detail}`), ...row.facts.filter((f) => f.k !== "trace" && f.k !== "input").map((f) => `${f.k}: ${f.v}`)];
  return (
    <div className="dgp" data-testid="prompt-row" data-input={row.input ?? ""}>
      <div className="dgp-h">
        <span className="h3">{row.input !== null ? `#${row.input}` : "Not recorded"}</span>
        <span className="faint small">{what}</span>
        <span className="sp" />
        {row.trace ? (
          <span className="faint mono small dgp-trace">
            {row.trace}
            <CopyButton text={row.trace} label="Copy trace id" />
          </span>
        ) : (
          <span className="faint small">no trace id</span>
        )}
      </div>
      <ol className="dgp-path" aria-label="Path of this prompt">
        {row.steps.map((s, i) => (
          <li key={s.id} className={`dgp-step ${s.state} ${s.tone}`} data-step={s.id} data-state={s.state} title={s.detail}>
            {i > 0 ? <Icon name="chevr" size="xs" className="dgp-sep" /> : null}
            <span className="dgp-chip">
              <span className="dgp-lbl">{s.label}</span>
              <span className="dgp-time mono tnum">{stepTime(s)}</span>
              {s.state === "seen" && s.source ? <span className="dgp-src">{s.source}</span> : null}
            </span>
          </li>
        ))}
      </ol>
      {row.steps.some((s) => s.state === "seen" && s.approx) ? <div className="faint small">≈ means another machine's clock: only approximately comparable to the first step.</div> : null}
      {notes.length ? (
        <ul className="dgp-notes faint small">
          {notes.map((n) => <li key={n}>{n}</li>)}
        </ul>
      ) : null}
    </div>
  );
}
