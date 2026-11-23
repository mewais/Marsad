import { memo } from "react";
import type { SessionKey, SessionSnapshot, SubagentInfo, TaskInfo } from "../../bridge/wire";
import { elapsed } from "../../lib/format";
import { Link, sessionPath } from "../../router";
import { stopTaskPendingKey, useActions, useAppState, usePending, useSessionReach, useSessionView } from "../../state";
import { Button, Icon, useNow } from "../../ui";
import { orderSubagents, otherBackgroundTasks, taskElapsedMs, taskLabel, taskState, type TaskState } from "./tasks";

/** How long a task or subagent has run (it ticks while it runs) or ran. */
export function Elapsed({ t }: { t: { started_at_ms: number; ended_at_ms?: number | null; running: boolean } }) {
  const now = useNow(t.running);
  return <>{elapsed(taskElapsedMs(t, now))}</>;
}

/** The icon and colour class of a task state. */
export const stateIcon = (s: TaskState) => ({ icon: s.tone === "working" ? "working" : s.tone === "failed" ? "failed" : "done", cls: `c-${s.tone}` }) as const;

/**
 * Stop one task the agent runs (`stop_task`). Offered only where the agent can: the capability says so. It shows
 * that the request is in flight and, if the daemon refuses it, says so (a toast); the task itself shows
 * as ended only when the daemon reports it ended.
 */
export function StopTask({ sessionKey, taskId, what }: { sessionKey: SessionKey; taskId: string; what: string }) {
  const actions = useActions();
  const supported = useAppState((s) => !!s.sessions[sessionKey]?.capabilities.stop_task);
  const pending = usePending(stopTaskPendingKey(sessionKey, taskId)) === "pending";
  const reach = useSessionReach(useSessionView(sessionKey));
  if (!supported) return null;
  return (
    <Button size="sm" variant="ghost" icon="stop" pending={pending} blocked={reach} aria-label={`Stop ${what}`} title={`Stop ${what}`} onClick={() => void actions.stopTask(sessionKey, taskId)}>
      Stop
    </Button>
  );
}

/** What a subagent is called: the agent's type for it, else its tool's name. */
export const subagentName = (s: Pick<SubagentInfo, "subagent_type" | "name">): string => s.subagent_type || s.name;

const calls = (n: number) => `${n} tool call${n === 1 ? "" : "s"}`;

const Subagent = memo(function Subagent({ sessionKey, host, id, s, selected }: { sessionKey: SessionKey; host: string; id: string; s: SubagentInfo; selected: boolean }) {
  const st = taskState(s);
  const { icon, cls } = stateIcon(st);
  const label = s.description || subagentName(s);
  return (
    <div className="wk" data-testid="side-subagent" data-running={s.running ? "" : undefined}>
      <Link className="wk-main" to={sessionPath(host, id, undefined, s.call_id)} aria-current={selected ? "page" : undefined} title={[s.model, s.description].filter(Boolean).join(" · ") || undefined}>
        <Icon name={icon} size="sm" spin={s.running} className={cls} />
        <b className="ell">{subagentName(s)}</b>
        <span className="ell faint">{s.description}</span>
        <span className="w">{st.word}</span>
      </Link>
      <div className="wk-meta faint">
        <span className="ell">{calls(s.tool_calls)} · <Elapsed t={s} /></span>
        <span className="flex1" />
        {s.running && s.task_id ? <StopTask sessionKey={sessionKey} taskId={s.task_id} what={`subagent ${label}`} /> : null}
      </div>
    </div>
  );
});

const Task = memo(function Task({ sessionKey, t }: { sessionKey: SessionKey; t: TaskInfo }) {
  return (
    <div className="wk" data-testid="side-task">
      <div className="wk-main">
        <Icon name="working" size="sm" spin className="c-working" />
        <b className="ell">{taskLabel(t)}</b>
        <span />
        <span className="w">Running</span>
      </div>
      <div className="wk-meta faint">
        <span className="ell mono">{t.kind}</span>
        <span className="ell">{t.last_tool ? `${t.last_tool} · ` : ""}<Elapsed t={t} /></span>
        <span className="flex1" />
        <StopTask sessionKey={sessionKey} taskId={t.task_id} what={`task ${taskLabel(t)}`} />
      </div>
    </div>
  );
});

/**
 * The work the agent runs besides its own turn, as the daemon reports it: its subagents (running first, then the
 * finished ones, each newest first; selecting one opens its transcript) and the background tasks that are not
 * subagents. Nothing is listed that the snapshot does not hold.
 */
export function SessionWork({ sessionKey, host, id, snap, selected }: { sessionKey: SessionKey; host: string; id: string; snap: Pick<SessionSnapshot, "subagents" | "tasks">; selected?: string }) {
  const subagents = orderSubagents(snap.subagents);
  const tasks = otherBackgroundTasks(snap);
  return (
    <>
      {subagents.length ? (
        <div>
          <h4>Subagents · {subagents.length}</h4>
          <div className="hist work" data-testid="side-subagents">
            {subagents.map((s) => <Subagent key={s.call_id} sessionKey={sessionKey} host={host} id={id} s={s} selected={s.call_id === selected} />)}
          </div>
        </div>
      ) : null}
      {tasks.length ? (
        <div>
          <h4>Background tasks · {tasks.length}</h4>
          <div className="hist work" data-testid="side-tasks">
            {tasks.map((t) => <Task key={t.task_id} sessionKey={sessionKey} t={t} />)}
          </div>
        </div>
      ) : null}
    </>
  );
}
