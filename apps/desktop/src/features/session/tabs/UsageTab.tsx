import { useMemo } from "react";
import type { SessionSnapshot } from "../../../bridge/wire";
import { clockTime, compactNumber, money } from "../../../lib/format";
import { useAppState, useDriver } from "../../../state";
import { Card, Icon } from "../../../ui";
import { usageFacts } from "./usage";

/** Usage tab: tokens, cost, context, per-model split and the CLI's compaction / rate-limit notices. */
export function UsageTab({ sessionKey, snap }: { sessionKey: string; snap: SessionSnapshot }) {
  const events = useAppState((s) => s.transcripts[sessionKey]?.events);
  const facts = useMemo(() => usageFacts(events ?? []), [events]);
  const drv = useDriver(snap.driver);
  const name = drv?.displayName ?? snap.driver;
  const u = snap.live_usage && snap.live_usage.input_tokens >= snap.usage.input_tokens ? snap.live_usage : snap.usage;
  const prompt = u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens;
  const cachePct = prompt ? Math.round((u.cache_read_tokens / prompt) * 100) : 0;
  const caps = snap.capabilities;
  return (
    <div className="usage">
      <div className="tiles">
        <Card className="tile"><span className="k">Cost so far</span><span className="v">{caps.cost ? money(u.cost_usd) : "n/a"}</span><span className="k">{caps.cost ? `${snap.turns_completed} turn${snap.turns_completed === 1 ? "" : "s"} completed` : `${name} does not report cost`}</span></Card>
        <Card className="tile"><span className="k">Input tokens</span><span className="v">{caps.usage ? compactNumber(prompt) : "n/a"}</span><span className="k">{caps.usage ? `${cachePct}% from cache` : `${name} does not report usage`}</span></Card>
        <Card className="tile"><span className="k">Output tokens</span><span className="v">{caps.usage ? compactNumber(u.output_tokens) : "n/a"}</span><span className="k">{compactNumber(u.cache_creation_tokens)} written to cache</span></Card>
        <Card className="tile"><span className="k">Context window</span><span className="v">{snap.context_pct != null ? `${Math.round(snap.context_pct)}%` : "n/a"}</span>
          {snap.context_pct != null ? <span className="bar ctxbar" role="meter" aria-label="Context window used" aria-valuenow={Math.round(snap.context_pct)} aria-valuemin={0} aria-valuemax={100}><i style={{ width: `${Math.min(100, snap.context_pct)}%` }} /></span> : <span className="k">not reported</span>}
        </Card>
      </div>
      {facts.compactions.length || facts.rateLimits.length ? (
        <div className="notices" role="list" aria-label="Notices from the agent">
          {facts.compactions.map((c, i) => (
            <div key={`c${i}`} className="note" role="listitem"><Icon name="layers" size="sm" /><span><b>History compacted</b> at {clockTime(c.t)} ({c.info.trigger}): {compactNumber(c.info.pre_tokens)} → {compactNumber(c.info.post_tokens)} tokens in {(c.info.duration_ms / 1000).toFixed(1)}s · {compactNumber(c.info.cumulative_dropped_tokens)} dropped so far</span></div>
          ))}
          {facts.rateLimits.map((r, i) => (
            <div key={`r${i}`} className="note warnnote" role="listitem"><Icon name="warn" size="sm" /><span><b>Rate limit: {r.info.status}</b> ({r.info.rate_limit_type}) · resets {clockTime(r.info.resets_at * 1000)}{r.info.is_using_overage ? " · using overage" : ""}</span></div>
          ))}
        </div>
      ) : null}
      <Card className="pad">
        <div className="h3">Per model</div>
        {facts.perModel ? (
          <table className="t"><thead><tr><th>Model</th><th className="r">Input</th><th className="r">Output</th><th className="r">Cost</th></tr></thead><tbody>
            {Object.entries(facts.perModel).map(([m, x]) => <tr key={m}><td className="mono">{m}</td><td className="r">{compactNumber(x.input_tokens)}</td><td className="r">{compactNumber(x.output_tokens)}</td><td className="r">{money(x.cost_usd)}{x.estimated ? " (est.)" : ""}</td></tr>)}
          </tbody></table>
        ) : <div className="faint small">{name} has not reported a per-model split for this session.</div>}
      </Card>
      {snap.subagents.length ? (
        <Card className="pad">
          <div className="h3">Subagents</div>
          <table className="t"><thead><tr><th>Subagent</th><th>Model</th><th>Task</th><th className="r">State</th></tr></thead><tbody>
            {snap.subagents.map((s) => <tr key={s.call_id}><td>{s.name}</td><td className="mono">{s.model ?? ""}</td><td>{s.description ?? ""}</td><td className="r">{s.running ? "Running" : s.ok === false ? "Failed" : "Finished"}</td></tr>)}
          </tbody></table>
        </Card>
      ) : null}
    </div>
  );
}
