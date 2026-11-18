import { memo } from "react";
import { agoShort, money } from "../../lib/format";
import { Button, DriverBadge, HostChip, ConnState, Icon, StatusPill } from "../../ui";
import { useDriver, useHostLink, useRenderProbe, useSessionView } from "../../state";
import { Gate } from "../gates/Gate";
import { DoneActions, FailedActions, NewDot, WorkstreamTag } from "../common/SessionBits";
import type { SessionKey } from "../../bridge/types";

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
  const host = useHostLink(v?.host ?? "");
  const drv = useDriver(v?.driver ?? "");
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
            <button type="button" className="nm" style={{ all: "unset", cursor: "pointer", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} onClick={(e) => { e.stopPropagation(); onOpen(v.key); }}>
              {v.name}
            </button>
            {v.workstream ? <WorkstreamTag name={v.workstream} /> : null}
          </div>
          {needs ? null : <div className={`s${v.status === "failed" ? " err" : ""}`}>{v.line}</div>}
        </div>
        <div>{drv ? <DriverBadge driver={drv} /> : v.driver}</div>
        <div style={{ display: "flex", flexDirection: "column", gap: 1, minWidth: 0 }}>
          {host ? (
            <>
              <HostChip host={host} />
              {host.link !== "connected" ? <ConnState host={host} short /> : null}
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
