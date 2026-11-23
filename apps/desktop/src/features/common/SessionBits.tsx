import { Button, Icon, Kbd } from "../../ui";
import { sessionPath, useNavigate } from "../../router";
import { continuePendingKey, notContinuableWords, useActions, usePending, useReach, useSessionReach, type SessionView } from "../../state";

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

/**
 * Continue an ended session. Whether it can be is the daemon's verdict on the snapshot (`continuable`):
 * nothing for a session whose process is running, enabled when it can, else disabled with the reason in
 * words (`why`: said next to the button; the session page says it in its own note).
 */
export function ContinueButton({ view, why = false }: { view: SessionView; why?: boolean }) {
  const actions = useActions();
  const blocked = useSessionReach(view);
  const pending = usePending(continuePendingKey(view.key)) === "pending";
  const c = view.cont;
  if (c.kind === "running") return null;
  const reason = c.kind === "blocked" ? notContinuableWords(c.why) : null;
  return (
    <>
      <Button size="sm" icon="refresh" pending={pending} blocked={blocked} disabled={!!reason} title={reason ?? "Start the agent again, with this session's earlier transcript"} onClick={() => void actions.continueSession(view.key)}>Continue</Button>
      {why && reason ? <span className="faint small" data-testid="continue-why">{reason}</span> : null}
    </>
  );
}

/** Actions for a failed session: continue it, open its log, ask the agent to fix. */
export function FailedActions({ view }: { view: SessionView }) {
  const actions = useActions();
  const nav = useNavigate();
  const blocked = useSessionReach(view);
  return (
    <>
      <ContinueButton view={view} why />
      <Button size="sm" variant="ghost" icon="term" onClick={() => nav(sessionPath(view.host, view.id))}>Open log</Button>
      <Button size="sm" variant="ghost" blocked={blocked} onClick={() => void actions.askToFix(view.key, view.line)}>Ask agent to fix</Button>
    </>
  );
}

/** A finished or ended session: what it changed, Continue when it has ended, and (when `seen`) marking it seen. */
export function DoneActions({ view, showStat = true, keys, seen = true }: { view: SessionView; showStat?: boolean; keys?: boolean; seen?: boolean }) {
  const actions = useActions();
  const nav = useNavigate();
  // Seen markers live in the hub's store: only the link to the hub matters, not the session's host.
  const blocked = useReach();
  return (
    <>
      {showStat && view.stat ? <DiffStat stat={view.stat} /> : null}
      <ContinueButton view={view} why />
      <Button size="sm" variant="ghost" icon="file" onClick={() => nav(sessionPath(view.host, view.id, "changes"))}>Review changes</Button>
      {seen ? (
        <Button size="sm" variant="ghost" icon="eye" blocked={blocked} onClick={() => actions.markSeen([view.key])}>
          Mark seen {keys ? <Kbd>E</Kbd> : null}
        </Button>
      ) : null}
    </>
  );
}


