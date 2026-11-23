import type { SessionSnapshot } from "../../bridge/wire";
import { clockTime, compactNumber, money } from "../../lib/format";
import { useDriver } from "../../state";
import { Icon } from "../../ui";
import { usageSummary } from "./usage";

const subagentState = (s: SessionSnapshot["subagents"][number]) => (s.running ? "Running" : s.ok === false ? "Failed" : "Finished");

/**
 * The session sidebar's facts: model, mode, cost, context window, tokens and cache, cost per
 * model, subagents. Everything comes from the snapshot, so it is as live as the snapshot is.
 */
export function SessionFacts({ snap }: { snap: SessionSnapshot }) {
  const drv = useDriver(snap.driver);
  const name = drv?.displayName ?? snap.driver;
  const u = usageSummary(snap);
  const ctx = u.contextPct == null ? null : Math.round(u.contextPct);
  return (
    <>
      <div>
        <h4>Session</h4>
        <dl className="kvl">
          <dt>Model</dt><dd className="mono">{snap.model ?? "n/a"}</dd>
          <dt>Mode</dt><dd>{snap.available_modes.find((m) => m.id === snap.mode)?.label ?? snap.mode ?? "n/a"}</dd>
          <dt>Cost so far</dt>
          <dd data-testid="side-cost">{u.reportsCost ? <>{money(u.cost)} <span className="faint">· {u.turns} turn{u.turns === 1 ? "" : "s"}</span></> : <span className="faint">{name} does not report cost</span>}</dd>
          <dt>Context window</dt>
          <dd data-testid="side-context">
            {ctx == null ? <span className="faint">not reported</span> : (
              <span className="ctx">
                <span className="bar ctxbar" role="meter" aria-label="Context window used" aria-valuenow={ctx} aria-valuemin={0} aria-valuemax={100}><i style={{ width: `${Math.min(100, ctx)}%` }} /></span>
                <span>{ctx}%</span>
              </span>
            )}
          </dd>
          <dt>Tokens in / out</dt>
          <dd data-testid="side-tokens">{u.reportsUsage ? `${compactNumber(u.promptTokens)} / ${compactNumber(u.outputTokens)}` : <span className="faint">{name} does not report usage</span>}</dd>
          {u.reportsUsage ? (
            <>
              <dt>Cache hits</dt><dd data-testid="side-cache">{u.cachePct}%</dd>
              <dt>Written to cache</dt><dd>{compactNumber(u.cacheWrittenTokens)}</dd>
            </>
          ) : null}
          <dt>Started</dt><dd>{clockTime(snap.created_at_ms)}</dd>
        </dl>
      </div>
      {u.perModel.length ? (
        <div>
          <h4>Cost by model</h4>
          <dl className="kvl" data-testid="side-per-model">
            {u.perModel.map(([m, x]) => (
              <div key={m} className="contents">
                <dt className="mono" title={`${compactNumber(x.input_tokens)} in / ${compactNumber(x.output_tokens)} out`}>{m}</dt>
                <dd>{money(x.cost_usd)}{x.estimated ? <span className="faint"> (est.)</span> : null}</dd>
              </div>
            ))}
          </dl>
        </div>
      ) : null}
      {snap.subagents.length ? (
        <div>
          <h4>Subagents · {snap.subagents.length}</h4>
          <div className="hist" data-testid="side-subagents">
            {snap.subagents.map((s) => (
              <div key={s.call_id} className="h" title={[s.model, s.description].filter(Boolean).join(" · ") || undefined}>
                <Icon name={s.running ? "working" : s.ok === false ? "failed" : "done"} size="sm" spin={s.running} className={s.running ? "c-working" : s.ok === false ? "c-failed" : "c-done"} />
                <span className="ell">{s.name}</span>
                <span className="w">{subagentState(s)}</span>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </>
  );
}
