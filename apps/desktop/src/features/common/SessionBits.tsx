import { Button, Icon, Kbd } from "../../ui";
import { sessionPath, useNavigate } from "../../router";
import { useActions, useReach, useSessionReach, type SessionView } from "../../state";

export function WorkstreamTag({ name }: { name: string }) {
  return (
    <span className="tag ws">
      <Icon name="graph" size="xs" />
      {name}
    </span>
  );
}

export const NewDot = () => <span className="newdot" title="Not seen yet" role="img" aria-label="Not seen yet" />;

export function DiffStat({ stat }: { stat: NonNullable<SessionView["stat"]> }) {
  return (
    <span className="stat mono fs115">
      {stat.files} files <span className="add">+{stat.added}</span> <span className="del">−{stat.removed}</span>
    </span>
  );
}

/** Actions for a failed session: continue it, open its log, ask the agent to fix. */
export function FailedActions({ view, compact }: { view: SessionView; compact?: boolean }) {
  const actions = useActions();
  const nav = useNavigate();
  const blocked = useSessionReach(view);
  return (
    <>
      <Button size="sm" icon="refresh" blocked={blocked} onClick={() => void actions.retry(view.key)}>Retry last step</Button>
      <Button size="sm" variant="ghost" icon="term" onClick={() => nav(sessionPath(view.host, view.id))}>Open log</Button>
      <Button size="sm" variant="ghost" blocked={blocked} onClick={() => void actions.askToFix(view.key, view.line)}>Ask agent to fix</Button>
      {compact ? null : null}
    </>
  );
}

export function DoneActions({ view, showStat = true, keys }: { view: SessionView; showStat?: boolean; keys?: boolean }) {
  const actions = useActions();
  const nav = useNavigate();
  // Seen markers live in the hub's store: only the link to the hub matters, not the session's host.
  const blocked = useReach();
  return (
    <>
      {showStat && view.stat ? <DiffStat stat={view.stat} /> : null}
      <Button size="sm" variant="ghost" icon="file" onClick={() => nav(sessionPath(view.host, view.id, "changes"))}>Review changes</Button>
      <Button size="sm" variant="ghost" icon="eye" blocked={blocked} onClick={() => actions.markSeen([view.key])}>
        Mark seen {keys ? <Kbd>E</Kbd> : null}
      </Button>
    </>
  );
}


