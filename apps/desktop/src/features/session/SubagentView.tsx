import { useState } from "react";
import type { SessionKey, SessionSnapshot } from "../../bridge/wire";
import { Link, sessionPath } from "../../router";
import { steerPendingKey, subagentScope, useActions, usePending, useSessionReach, useSessionView } from "../../state";
import { Button, Icon, StatusPill, type DriverDescriptor } from "../../ui";
import { StopTask, SubagentMeta, subagentName } from "./SessionWork";
import { subagentState } from "./tasks";
import { TranscriptList } from "./TranscriptList";

function subagentWords(snap: Pick<SessionSnapshot, "subagents">, callId: string) {
  const info = snap.subagents.find((s) => s.call_id === callId);
  const name = info ? subagentName(info) : "Subagent";
  const whose = info ? `the subagent “${info.description || name}” (${name})` : "a subagent";
  return { info, name, whose };
}

/**
 * A subagent's own transcript, in place of the session's: who it is and how it stands (as the daemon reports), the
 * way back, and its conversation (the session's transcript, parameterised by its scope).
 */
export function SubagentView({ sessionKey, host, id, callId, snap, driver }: { sessionKey: string; host: string; id: string; callId: string; snap: Pick<SessionSnapshot, "subagents" | "capabilities">; driver?: DriverDescriptor }) {
  const { info, name, whose } = subagentWords(snap, callId);
  const st = info ? subagentState(info) : undefined;
  return (
    <>
      <div className="subbar" data-testid="subagent-bar">
        <Link className="btn sm ghost" to={sessionPath(host, id)} data-testid="subagent-back"><Icon name="chev" size="xs" className="back" />Back to the session</Link>
        <Icon name="sub" size="sm" />
        <b>Subagent · {name}</b>
        {info?.description ? <span className="muted ell">“{info.description}”</span> : null}
        <span className="flex1" />
        {st ? <span title={st.why}><StatusPill status={st.status} label={st.word} /></span> : null}
        {info ? <span className="faint"><SubagentMeta s={info} caps={snap.capabilities.subagents} /></span> : null}
        {info?.running && info.task_id ? <StopTask sessionKey={sessionKey} taskId={info.task_id} what={`subagent ${info.description || name}`} /> : null}
      </div>
      <TranscriptList sessionKey={sessionKey} host={host} id={id} scope={subagentScope(callId)} driver={driver} empty={`Nothing from ${whose} yet.`} />
    </>
  );
}

/**
 * Where the composer would be. A running subagent whose driver can be steered takes a message (it reads it at its next
 * step, and the session's own turn is untouched); any other subagent's transcript is read only, and says so.
 */
export function SubagentFoot({ sessionKey, snap, callId }: { sessionKey: SessionKey; snap: Pick<SessionSnapshot, "subagents" | "capabilities">; callId: string }) {
  const { info, whose } = subagentWords(snap, callId);
  if (info?.running && info.task_id && snap.capabilities.subagents.steer) return <SteerSubagent sessionKey={sessionKey} taskId={info.task_id} whose={whose} />;
  return (
    <div className="subfoot faint fs12" data-testid="subagent-readonly" role="note">
      <Icon name="lock" size="xs" />
      {info ? "" : "This session has no record of that subagent. "}This is the transcript of {whose}. It is read only: messages go to the session’s own agent, in the session.
    </div>
  );
}

/** The message box for a running subagent: `steer_subagent` on the parent session, for this task. */
function SteerSubagent({ sessionKey, taskId, whose }: { sessionKey: SessionKey; taskId: string; whose: string }) {
  const actions = useActions();
  const [text, setText] = useState("");
  const pending = usePending(steerPendingKey(sessionKey, taskId)) === "pending";
  const reach = useSessionReach(useSessionView(sessionKey));
  const send = async () => {
    const message = text.trim();
    if (!message || pending) return;
    // The box keeps the text when the daemon refuses it, so nothing typed is lost.
    if (await actions.steerSubagent(sessionKey, taskId, message)) setText("");
  };
  return (
    <form className="subfoot substeer" data-testid="subagent-steer" aria-label={`Message to ${whose}`} onSubmit={(e) => { e.preventDefault(); void send(); }}>
      <input
        className="substeer-input"
        aria-label="Message this sub-agent"
        placeholder="Message this sub-agent…"
        value={text}
        disabled={!!reach}
        onChange={(e) => setText(e.target.value)}
      />
      <Button type="submit" size="sm" pending={pending} blocked={reach} disabled={!text.trim()}>Send</Button>
      <span className="faint fs12">It reads your message at its next step.</span>
    </form>
  );
}
