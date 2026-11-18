import { memo, useEffect } from "react";
import { agoShort, money } from "../../lib/format";
import { Button, DriverBadge, HostChip, ConnState, Icon, StatusPill } from "../../ui";
import { linkView, useActions, useDriver, useHost, useRenderProbe, useSessionView } from "../../state";
import { Gate } from "../gates/Gate";
import { DoneActions, FailedActions, NewDot, WorkstreamTag } from "../common/SessionBits";
import type { SessionKey } from "../../bridge/wire";

export interface InboxRowProps {
  sessionKey: SessionKey;
  selected: boolean;
  onSelect: (key: SessionKey) => void;
  onOpen: (key: SessionKey) => void;
}

/** One Inbox row. Subscribes only to its own session, so other sessions' updates do not re-render it. */
export const InboxRow = memo(function InboxRow({ sessionKey, selected, onSelect, onOpen }: InboxRowProps) {
  useRenderProbe(`row:${sessionKey}`);
  const v = useSessionView(sessionKey);
  const host = useHost(v?.host ?? "");
  const drv = useDriver(v?.driver ?? "");
  const actions = useActions();
  // Branch and diffstat are not in the snapshot: ask git once the row is on screen.
  useEffect(() => actions.ensureGit(sessionKey), [actions, sessionKey]);
  if (!v) return null;
  const needs = v.status === "needs";
  const gate = needs ? v.gates[0] : undefined;
  const activate = () => (needs ? onSelect(v.key) : onOpen(v.key));
  return (
    <div className={`ibx-item${selected ? " sel" : ""}`} role="listitem" data-session={v.key} data-status={v.status} aria-current={selected ? "true" : undefined}>
      <div className="ibx-row" onClick={activate} onDoubleClick={() => onOpen(v.key)}>
        <div><StatusPill status={v.status} /></div>
        <div className="sn">
          <div className="t">
            {v.unseen ? <NewDot /> : null}
            <button type="button" className="nm unsetbtn" onClick={(e) => { e.stopPropagation(); onOpen(v.key); }}>
              {v.name}
            </button>
            {v.workstream ? <WorkstreamTag name={v.workstream} /> : null}
          </div>
          {needs ? null : <div className={`s${v.status === "failed" ? " err" : ""}`}>{v.line}</div>}
        </div>
        <div>{drv ? <DriverBadge driver={drv} /> : v.driver}</div>
        <div className="col1">
          {host ? (
            <>
              <HostChip host={host} />
              {!linkView(host.link, host.authPrompt).connected ? <ConnState host={host} short /> : null}
            </>
          ) : null}
        </div>
        <div className="pb">
          <span className="p">{v.project}</span>
          <span className="b"><Icon name="branch" size="xs" /> {v.branch}</span>
        </div>
        <div className="ago r">{agoShort(v.lastMs)}</div>
        <div className="cost r">{money(v.cost)}</div>
      </div>
      {gate ? (
        <>
          <Gate view={v} pg={gate} density="row" active={selected} expanded={selected} onActivate={() => onSelect(v.key)} />
          {v.gates.length > 1 ? (
            <div className="donebox">
              <Button size="sm" variant="ghost" onClick={() => onOpen(v.key)}>+{v.gates.length - 1} more waiting · open session <Icon name="chevr" size="xs" /></Button>
            </div>
          ) : null}
        </>
      ) : null}
      {v.status === "done" ? <div className="donebox"><DoneActions view={v} keys={selected} /></div> : null}
      {v.status === "failed" ? <div className="donebox"><FailedActions view={v} /></div> : null}
    </div>
  );
});
