import type { SessionSnapshot } from "../../bridge/wire";
import { Link, sessionPath } from "../../router";
import { subagentScope } from "../../state";
import { Icon, StatusPill, type DriverDescriptor } from "../../ui";
import { Elapsed, StopTask, subagentName } from "./SessionWork";
import { taskState } from "./tasks";
import { TranscriptList } from "./TranscriptList";

/**
 * A subagent's own transcript, in place of the session's: who it is and how it stands (as the daemon reports), the
 * way back, its conversation (the session's transcript, parameterised by its scope) and, where the composer would be, the
 * words that it is read-only and whose it is. Nothing here takes a message.
 */
export function SubagentView({ sessionKey, host, id, callId, snap, driver }: { sessionKey: string; host: string; id: string; callId: string; snap: Pick<SessionSnapshot, "subagents">; driver?: DriverDescriptor }) {
  const info = snap.subagents.find((s) => s.call_id === callId);
  const st = info ? taskState(info) : undefined;
  const name = info ? subagentName(info) : "Subagent";
  const whose = info ? `the subagent “${info.description || name}” (${name})` : "a subagent";
  return (
    <>
      <div className="subbar" data-testid="subagent-bar">
        <Link className="btn sm ghost" to={sessionPath(host, id)} data-testid="subagent-back"><Icon name="chev" size="xs" className="back" />Back to the session</Link>
        <Icon name="sub" size="sm" />
        <b>Subagent · {name}</b>
        {info?.description ? <span className="muted ell">“{info.description}”</span> : null}
        <span className="flex1" />
        {st ? <StatusPill status={st.tone} label={st.word} /> : null}
        {info ? <span className="faint">{info.tool_calls} tool call{info.tool_calls === 1 ? "" : "s"} · <Elapsed t={info} /></span> : null}
        {info?.running && info.task_id ? <StopTask sessionKey={sessionKey} taskId={info.task_id} what={`subagent ${info.description || name}`} /> : null}
      </div>
      <TranscriptList sessionKey={sessionKey} host={host} id={id} scope={subagentScope(callId)} driver={driver} empty={`Nothing from ${whose} yet.`} />
      <div className="subfoot faint fs12" data-testid="subagent-readonly" role="note">
        <Icon name="lock" size="xs" />
        {info ? "" : "This session has no record of that subagent. "}This is the transcript of {whose}. It is read only: messages go to the session’s own agent, in the session.
      </div>
    </>
  );
}
