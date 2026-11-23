import { useMemo, useState } from "react";
import type { SessionSnapshot } from "../../bridge/wire";
import { splitKey } from "../../bridge/wire";
import { useHost, useAppState, useReach, type TailSpec } from "../../state";
import { Button, Icon, Skeleton, useCopy } from "../../ui";
import { DiagList } from "./DiagList";
import { useReconnect, useTail, useTrail } from "./hooks";
import { Facts, Problem, Reading, SectionHead, Unreachable } from "./parts";
import { PromptPath } from "./PromptPath";
import { promptRows, promptTraces } from "./prompts";
import { recordRows, trailRows } from "./rows";
import { panelText } from "./text";

/**
 * The Diagnostics tab of a session (Developer mode): where its prompts went, how its state
 * changed, and what its host logged about it. Everything is read from the daemons while the tab
 * is open (and the host reachable) and forgotten when it is closed. Nothing here is content: ids,
 * sizes, timings, states and reasons, from the trail and the logs, never joined with the transcript.
 */
export function SessionDiagnostics({ sessionKey, snap }: { sessionKey: string; snap: SessionSnapshot }) {
  const { host, id } = splitKey(sessionKey);
  const hostView = useHost(host);
  const local = hostView?.local ?? false;
  const hostName = hostView?.name ?? host;
  // Nothing is asked of a daemon that cannot be reached; the panel says so and reads again when it can.
  const reach = useReach(host);
  const reconnect = useReconnect(host);
  const trail = useTrail(sessionKey, !reach);
  const items = trail.slice?.items;
  const appEvents = useAppState((s) => s.appEvents);
  const diagLog = useAppState((s) => s.diagLog);

  // Prompt legs on the hub and on the host come from their logs, asked for by the prompts' traces.
  const traces = useMemo(() => promptTraces(items ?? [], appEvents, { host, id }), [items, appEvents, host, id]);
  const tracesKey = traces.join(",");
  const hubSpec = useMemo<TailSpec | null>(() => (!reach && traces.length ? { id: `prompts:hub:${sessionKey}:${tracesKey}`, filter: { traces } } : null), [reach, sessionKey, tracesKey, traces]);
  const hostSpec = useMemo<TailSpec | null>(() => (!reach && !local && traces.length ? { id: `prompts:host:${sessionKey}:${tracesKey}`, host, filter: { traces } } : null), [reach, local, host, sessionKey, tracesKey, traces]);
  const hubTail = useTail(hubSpec);
  const hostTail = useTail(hostSpec);
  const hubLoaded = !reach && (!hubSpec || hubTail.slice?.state === "live");
  const hostLoaded = !reach && (!hostSpec || hostTail.slice?.state === "live");

  const prompts = useMemo(
    () =>
      promptRows({
        trail: items ?? [],
        app: appEvents,
        hub: hubTail.slice?.records ?? [],
        host: hostTail.slice?.records ?? [],
        session: { host, id, hostName, local },
        loaded: { hub: hubLoaded, host: hostLoaded },
      }),
    [items, appEvents, hubTail.slice, hostTail.slice, host, id, hostName, local, hubLoaded, hostLoaded],
  );
  const timeline = useMemo(() => trailRows(items ?? []), [items]);
  /** The trail has been asked for and nothing has come back (not an empty trail: that is an answer). */
  const trailPending = !reach && (!trail.slice || (trail.slice.loading && !items?.length));

  const [logOpen, setLogOpen] = useState(false);
  const logSpec = useMemo<TailSpec | null>(() => (logOpen && !reach ? { id: `session-log:${sessionKey}`, host: local ? undefined : host, filter: { session: id } } : null), [logOpen, reach, local, host, id, sessionKey]);
  const log = useTail(logSpec);
  const logRows = useMemo(() => recordRows(log.slice?.records ?? []), [log.slice?.records]);

  const facts = useMemo(
    () => [
      { k: "Session id", v: id },
      { k: "Host", v: hostView ? `${hostName} (${host})` : host },
      { k: "Driver", v: snap.driver },
      { k: "Generation", v: String(snap.generation) },
      { k: "Native session id", v: snap.native_session_id ?? null },
      { k: "Phase", v: snap.phase.state },
      { k: "CLI version", v: snap.cli_version ?? null },
      { k: "Model", v: snap.model ?? null },
    ],
    [id, host, hostName, hostView, snap.driver, snap.generation, snap.native_session_id, snap.phase.state, snap.cli_version, snap.model],
  );
  const text = useMemo(
    () => panelText({ facts, prompts, trail: items ?? null, trailHasMore: !!trail.slice?.hasMore, hostLog: log.slice?.state === "live" ? log.slice.records : null, at: Date.now() }),
    [facts, prompts, items, trail.slice?.hasMore, log.slice],
  );
  const copy = useCopy(text);

  const top = (
    <div className="dg-top">
      <Facts facts={facts} />
      <div className="dg-copy">
        <Button size="sm" icon={copy.state === "copied" ? "check" : "copy"} onClick={copy.copy}>{copy.state === "copied" ? "Copied" : copy.state === "failed" ? "Copy failed" : "Copy as text"}</Button>
        <span className="faint small">Ids, sizes, timings and reasons only; no prompts, replies or files.</span>
      </div>
    </div>
  );
  // Nothing is read from a daemon that cannot be reached: the facts the app has, and why the rest is not here.
  if (reach)
    return (
      <div className="dg" data-testid="session-diagnostics">
        {top}
        <Unreachable reason={reach} retry={reconnect} />
      </div>
    );

  return (
    <div className="dg" data-testid="session-diagnostics">
      {top}
      {diagLog.failed ? <Problem title={`${diagLog.failed} app event${diagLog.failed === 1 ? "" : "s"} could not be written to the app's log.`} detail={diagLog.error ?? ""} /> : null}

      <section aria-label="Prompts">
        <SectionHead title="Prompts" note="Each step is shown only where something observed it. Times are relative to the first step." />
        {trailPending ? <Reading what="Reading the trail from the host…" /> : null}
        {prompts.map((r) => <PromptPath key={r.key} row={r} />)}
        {!prompts.length && !trailPending && !trail.slice?.error ? <div className="faint small">No prompts yet.</div> : null}
        {trail.slice?.hasMore ? <div className="faint small">Only the loaded part of the trail is searched for prompts; load earlier entries below for older ones.</div> : null}
        {hubTail.feedError ? <Problem title="The hub's log stopped." detail={hubTail.feedError} onRetry={hubTail.retry} /> : null}
        {hostTail.feedError ? <Problem title={`${hostName}'s log stopped.`} detail={hostTail.feedError} onRetry={hostTail.retry} /> : null}
      </section>

      <section aria-label="Timeline">
        <SectionHead title="Timeline" note="The host's trail, in the host's order and clock. Newest at the bottom.">
          {trail.slice?.hasMore ? <Button size="sm" variant="ghost" pending={trail.slice.loading} onClick={trail.loadEarlier}>Load earlier</Button> : null}
        </SectionHead>
        {trail.slice?.error ? <Problem title="Could not read the trail." detail={trail.slice.error} onRetry={trail.retry} /> : null}
        {trail.feedError ? <Problem title="The live trail stopped." detail={trail.feedError} onRetry={trail.retry} /> : null}
        {trailPending ? (
          <div className="dg-skel"><Skeleton width="70%" /><Skeleton /><Skeleton width="80%" /></div>
        ) : timeline.length ? (
          <DiagList rows={timeline} label="Session trail" height={340} />
        ) : trail.slice && !trail.slice.error ? (
          <div className="faint small">The host has no trail entries for this session yet.</div>
        ) : null}
      </section>

      <section aria-label="Host log for this session">
        <button type="button" className="dg-toggle" aria-expanded={logOpen} onClick={() => setLogOpen((o) => !o)}>
          <Icon name={logOpen ? "chev" : "chevr"} size="xs" />
          <span className="eyebrow">Host log for this session</span>
          <span className="faint small">{local ? "this computer's service" : hostName}</span>
        </button>
        {logOpen ? (
          <>
            {log.feedError ? <Problem title="The log stopped." detail={log.feedError} onRetry={log.retry} /> : null}
            {!log.slice || log.slice.state === "waiting" ? (
              <Reading what={`Reading ${local ? "the local service's" : `${hostName}'s`} log…`} />
            ) : log.slice.records.length ? (
              <DiagList rows={logRows} label="Host log for this session" height={240} />
            ) : (
              <div className="faint small">The daemon has logged nothing about this session since it started (or its log has moved on).</div>
            )}
            {log.slice?.unreadable ? <div className="faint small">{log.slice.unreadable} lines of the log could not be read.</div> : null}
          </>
        ) : null}
      </section>
    </div>
  );
}
