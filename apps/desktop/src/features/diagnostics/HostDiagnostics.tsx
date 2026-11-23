import { useEffect, useMemo, useRef, useState } from "react";
import type { HostView } from "../../state";
import { linkView, useReach, type TailSpec } from "../../state";
import { Button } from "../../ui";
import { clockMs, duration, recordTone, recordWords } from "./words";
import { connectionAttempts, type Attempt } from "./attempts";
import { DiagList } from "./DiagList";
import { useReconnect, useTail } from "./hooks";
import { Problem, Reading, SectionHead, Unreachable } from "./parts";
import { recordRows } from "./rows";

/** Daemon, bridge, environment, discovery and runtime records, and a session being lost or retried: what a host daemon says about itself. */
const ON_THE_HOST = ["daemon.", "bridge.", "env.", "discovery.", "runtime.", "session.lost", "session.retry"] as const;
/** How many attempts the connection log lists at first. */
const ATTEMPTS_SHOWN = 12;

const OUTCOME: Record<string, { tone: "ok" | "warn" | "bad" | "info"; word: string }> = {
  connected: { tone: "ok", word: "Connected" },
  backoff: { tone: "warn", word: "Failed, will retry" },
  park: { tone: "bad", word: "Failed, waiting for you" },
  provision: { tone: "info", word: "Needs the helper installed or updated" },
  immediate: { tone: "warn", word: "Failed, retrying at once" },
};

function AttemptBlock({ a }: { a: Attempt }) {
  const t0 = a.begin ?? a.records[0]?.t ?? 0;
  const outcome = a.outcome ? OUTCOME[a.outcome] ?? { tone: "info" as const, word: a.outcome } : a.n === null ? null : { tone: "info" as const, word: "In progress" };
  const failed = a.outcome === "backoff" || a.outcome === "park" || a.outcome === "immediate" || !!a.failure;
  return (
    <div className={`dga ${failed ? "bad" : a.outcome === "connected" ? "ok" : ""}`} data-testid="attempt" data-attempt={a.n ?? "between"}>
      <div className="dga-h">
        <b>{a.n === null ? "Between attempts" : `Attempt ${a.n}`}</b>
        <span className="faint small mono tnum">{clockMs(t0)}</span>
        {outcome ? <span className={`conn ${outcome.tone}`}><span className="pip" />{outcome.word}</span> : null}
        {a.ms !== undefined ? <span className="faint small tnum">{duration(a.ms)}</span> : null}
      </div>
      <ul className="dga-steps">
        {a.records.map((r) => (
          <li key={r.id} className={`dga-step ${recordTone(r)}`}>
            <span className="faint mono tnum dga-t">{a.n === null ? clockMs(r.t) : `+${duration(r.t - t0)}`}</span>
            <span>{recordWords(r)}</span>
          </li>
        ))}
      </ul>
      {a.failure ? (
        <div className="dga-fail" role="note">
          <div>
            Failed at <b>{a.failure.stage ?? "an unknown stage"}</b>
            {a.failure.kind ? <> ({a.failure.kind})</> : null}
            {a.failure.exit !== undefined ? <> · exit code <b>{a.failure.exit}</b></> : null}
          </div>
          {a.failure.stderr ? <pre className="mono dga-err" aria-label="What the host's shell wrote (scrubbed)">{a.failure.stderr}</pre> : null}
          {a.failure.error ? <div className="small">{a.failure.error}</div> : null}
        </div>
      ) : a.error ? (
        <div className="dga-fail small" role="note">{a.error}</div>
      ) : null}
    </div>
  );
}

/** A scroll box that stays at its bottom (the newest) while it grows, unless the reader scrolled up. */
function useStickBottom<T extends HTMLElement>(changes: unknown) {
  const ref = useRef<T>(null);
  const stick = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [changes]);
  return { ref, onScroll: (e: React.UIEvent<T>) => (stick.current = e.currentTarget.scrollHeight - e.currentTarget.scrollTop - e.currentTarget.clientHeight < 40) };
}

/** The hub's records about one host: how it connects and why it does not. */
function ConnectionLog({ host }: { host: HostView }) {
  const reach = useReach();
  const reconnect = useReconnect();
  const spec = useMemo<TailSpec | null>(() => (reach ? null : { id: `connection:${host.id}`, filter: {}, keep: { about: host.id } }), [reach, host.id]);
  const tail = useTail(spec);
  const [all, setAll] = useState(false);
  const attempts = useMemo(() => connectionAttempts(tail.slice?.records ?? []), [tail.slice?.records]);
  const shown = all ? attempts : attempts.slice(-ATTEMPTS_SHOWN);
  const stick = useStickBottom<HTMLDivElement>(tail.slice?.records[tail.slice.records.length - 1]?.id);
  return (
    <section aria-label="Connection log" data-testid="connection-log">
      <SectionHead title="Connection log" note="From the hub, by attempt." />
      {reach ? <Unreachable reason={reach} retry={reconnect} /> : null}
      {tail.feedError ? <Problem title="The connection log stopped." detail={tail.feedError} onRetry={tail.retry} /> : null}
      {!reach && (!tail.slice || tail.slice.state === "waiting") ? <Reading what="Reading the hub's log…" /> : null}
      {tail.slice?.state === "live" && !attempts.length ? <div className="faint small">The hub has no connection records for {host.name} since it started.</div> : null}
      {attempts.length > shown.length ? <Button size="sm" variant="ghost" onClick={() => setAll(true)}>Show {attempts.length - shown.length} earlier</Button> : null}
      <div className="dga-list" ref={stick.ref} onScroll={stick.onScroll}>{shown.map((a, i) => <AttemptBlock key={`${a.n}:${a.records[0]?.id ?? i}`} a={a} />)}</div>
      {tail.slice?.trimmed ? <div className="faint small">{tail.slice.trimmed} older records are not kept here.</div> : null}
    </section>
  );
}

/** A daemon's own log as a list: the hub's, or a host's filtered to what a host says about itself. */
function LogSection({ title, note, spec, reach, reconnect, empty }: { title: string; note: string; spec: TailSpec | null; reach: string | null; reconnect: ReturnType<typeof useReconnect>; empty: string }) {
  const tail = useTail(spec);
  const rows = useMemo(() => recordRows(tail.slice?.records ?? [], true), [tail.slice?.records]);
  return (
    <section aria-label={title}>
      <SectionHead title={title} note={note} />
      {reach ? <Unreachable reason={reach} retry={reconnect} /> : null}
      {tail.feedError ? <Problem title="The log stopped." detail={tail.feedError} onRetry={tail.retry} /> : null}
      {!reach && (!tail.slice || tail.slice.state === "waiting") ? <Reading what="Reading the log…" /> : null}
      {tail.slice?.state === "live" ? rows.length ? <DiagList rows={rows} label={title} height={260} /> : <div className="faint small">{empty}</div> : null}
      {tail.slice?.trimmed ? <div className="faint small">{tail.slice.trimmed} older records are not kept here.</div> : null}
      {tail.slice?.unreadable ? <div className="faint small">{tail.slice.unreadable} lines of the log could not be read.</div> : null}
    </section>
  );
}

/** Developer mode's section of a host: its connection log and, when it is connected, its own log. For the local host there is no link: it shows the hub daemon's log. */
export function HostDiagnostics({ host }: { host: HostView }) {
  const hubReach = useReach();
  const hostReach = useReach(host.id);
  const hubReconnect = useReconnect();
  const hostReconnect = useReconnect(host.id);
  const hubSpec = useMemo<TailSpec | null>(() => (hubReach || !host.local ? null : { id: "hub-log", filter: {} }), [hubReach, host.local]);
  const onHostSpec = useMemo<TailSpec | null>(() => (hostReach || host.local ? null : { id: `onhost:${host.id}`, host: host.id, filter: {}, keep: { events: ON_THE_HOST } }), [hostReach, host.local, host.id]);
  const connected = linkView(host.link, host.authPrompt).connected;
  return (
    <div className="dg dg-host" data-testid="host-diagnostics">
      <h4 className="eyebrow">Diagnostics</h4>
      {host.local ? (
        <>
          <div className="faint small" role="note">This is the local host: this app reaches it through the local service (the hub), so there is no connection to log. This is the hub daemon's own log.</div>
          <LogSection title="Local service log" note="From this computer's daemon." spec={hubSpec} reach={hubReach} reconnect={hubReconnect} empty="The local service has logged nothing yet." />
        </>
      ) : (
        <>
          <ConnectionLog host={host} />
          {connected ? <LogSection title="On the host" note={`${host.name}'s own daemon.`} spec={onHostSpec} reach={hostReach} reconnect={hostReconnect} empty={`${host.name} has logged nothing of this kind yet.`} /> : <div className="faint small">{host.name}'s own log is read while it is connected.</div>}
        </>
      )}
    </div>
  );
}
