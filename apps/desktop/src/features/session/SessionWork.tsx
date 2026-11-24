import { memo } from "react";
import type { SessionKey, SessionSnapshot, SubagentInfo, TaskInfo } from "../../bridge/wire";
import { elapsed } from "../../lib/format";
import { Link, sessionPath } from "../../router";
import { stopTaskPendingKey, useActions, useAppState, usePending, useSessionReach, useSessionView } from "../../state";
import { Button, Icon, useNow } from "../../ui";
import { otherBackgroundTasks, taskElapsedMs, taskLabel, taskState, type TaskState } from "./tasks";

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

export const SubagentRow = memo(function SubagentRow({ sessionKey, host, id, s, selected }: { sessionKey: SessionKey; host: string; id: string; s: SubagentInfo; selected: boolean }) {
  const st = taskState(s);
  const { icon, cls } = stateIcon(st);
  const label = s.description || subagentName(s);
  return (
    <div className="subrow" data-testid="subagent-strip-row" data-running={s.running ? "" : undefined} data-selected={selected ? "" : undefined}>
      <Icon name={icon} size="sm" spin={s.running} className={cls} />
      <b className="subrow-name ell">{subagentName(s)}</b>
      <span className="subrow-desc ell" title={[s.model, s.description].filter(Boolean).join(" · ") || undefined}>{s.description}</span>
      <span className={`subrow-st ${cls}`}>{st.word}</span>
      <span className="subrow-meta faint tnum">{calls(s.tool_calls)} · <Elapsed t={s} /></span>
      <span className="subrow-acts">
        {s.running && s.task_id ? <StopTask sessionKey={sessionKey} taskId={s.task_id} what={`subagent ${label}`} /> : null}
        <Link
          className="btn sm ghost"
          to={sessionPath(host, id, undefined, s.call_id)}
          aria-current={selected ? "page" : undefined}
          aria-label={`Open transcript for ${label}`}
        >
          <Icon name="chevr" size="xs" />Open transcript
        </Link>
      </span>
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
 * Background tasks running in this session that are not subagents (subagents live in the expandable strip above the
 * composer). Nothing is listed that the snapshot does not hold.
 */
export function SessionWork({ sessionKey, snap }: { sessionKey: SessionKey; snap: Pick<SessionSnapshot, "subagents" | "tasks"> }) {
  const tasks = otherBackgroundTasks(snap);
  if (!tasks.length) return null;
  return (
    <div>
      <h4>Background tasks · {tasks.length}</h4>
      <div className="hist work" data-testid="side-tasks">
        {tasks.map((t) => <Task key={t.task_id} sessionKey={sessionKey} t={t} />)}
      </div>
    </div>
  );
}
