import { memo } from "react";
import type { SessionKey, SessionSnapshot, SubagentInfo, SubagentSupport, TaskInfo } from "../../bridge/wire";
import { Link, sessionPath } from "../../router";
import { stopTaskPendingKey, useActions, useAppState, usePending, useSessionReach, useSessionView } from "../../state";
import { Button, Elapsed, Icon, STATUS } from "../../ui";
import { otherBackgroundTasks, subagentFacts, subagentState, taskLabel, type SubagentState } from "./tasks";

/** The icon and colour class of a subagent's state. Stopped and unknown read as neutral, not as a failure or a success. */
export const subagentIcon = (st: SubagentState) => {
  const cls = st.status === "working" ? "c-working" : st.status === "done" ? "c-done" : st.status === "failed" ? "c-failed" : "faint";
  return { icon: STATUS[st.status].icon, cls } as const;
};

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

/**
 * What a subagent has used, as its facts and how long it has run (or ran): the figures the driver's capabilities say it
 * reports, and none it does not.
 */
export function SubagentMeta({ s, caps }: { s: SubagentInfo; caps: SubagentSupport }) {
  return (
    <>
      {subagentFacts(s, caps).map((f) => <span key={f}>{f} · </span>)}
      <Elapsed t={s} />
    </>
  );
}

export const SubagentRow = memo(function SubagentRow({ sessionKey, host, id, s, selected, caps }: { sessionKey: SessionKey; host: string; id: string; s: SubagentInfo; selected: boolean; caps: SubagentSupport }) {
  const st = subagentState(s);
  const { icon, cls } = subagentIcon(st);
  const label = s.description || subagentName(s);
  return (
    <div className="subrow" data-testid="subagent-strip-row" data-running={s.running ? "" : undefined} data-selected={selected ? "" : undefined}>
      <Icon name={icon} size="sm" spin={s.running} className={cls} />
      <b className="subrow-name ell">{subagentName(s)}</b>
      <span className="subrow-desc ell" title={[s.model, s.description].filter(Boolean).join(" · ") || undefined}>{s.description}</span>
      <span className={`subrow-st ${cls}`} title={st.why}>{st.word}</span>
      <span className="subrow-meta faint tnum"><SubagentMeta s={s} caps={caps} /></span>
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
