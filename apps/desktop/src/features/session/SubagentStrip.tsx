import { useId } from "react";
import type { SessionKey, SessionSnapshot } from "../../bridge/wire";
import { Icon, Segmented } from "../../ui";
import { SubagentRow } from "./SessionWork";
import { orderSubagents } from "./tasks";

export type SubagentFilter = "active" | "finished" | "all";

const FILTERS: { value: SubagentFilter; label: string }[] = [
  { value: "active", label: "Active" },
  { value: "finished", label: "Finished" },
  { value: "all", label: "All" },
];

/**
 * Expandable strip directly above the composer showing the session's subagents.
 * Hidden when the session has none; collapsed by default to a single summary line.
 */
export function SubagentStrip({
  sessionKey,
  host,
  id,
  snap,
  selected,
  open,
  onOpenChange,
  filter,
  onFilterChange,
}: {
  sessionKey: SessionKey;
  host: string;
  id: string;
  snap: Pick<SessionSnapshot, "subagents" | "capabilities">;
  selected?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  filter: SubagentFilter | null;
  onFilterChange: (filter: SubagentFilter) => void;
}) {
  const panelId = useId();
  const subagents = orderSubagents(snap.subagents);
  if (!subagents.length) return null;

  const working = subagents.filter((s) => s.running).length;
  const finished = subagents.length - working;
  const effectiveFilter: SubagentFilter = filter ?? (working > 0 ? "active" : "all");
  const visible = effectiveFilter === "active"
    ? subagents.filter((s) => s.running)
    : effectiveFilter === "finished"
      ? subagents.filter((s) => !s.running)
      : subagents;

  return (
    <section className="substrip-wrap" aria-label="Subagents">
      <div className="substrip" data-testid="subagent-strip" data-open={open ? "" : undefined}>
        <div className="substrip-head">
          <button
            type="button"
            className="substrip-toggle plainbtn"
            data-testid="subagent-strip-toggle"
            aria-expanded={open}
            aria-controls={panelId}
            onClick={() => onOpenChange(!open)}
          >
            <Icon name={open ? "chev" : "chevr"} size="xs" className="faint" />
            <Icon name={working > 0 ? "working" : "sub"} size="sm" spin={working > 0} className={working > 0 ? "c-working" : "faint"} />
            <span className="substrip-summary ell">
              <b className="fg">Subagents</b>
              {working > 0 ? <>{" · "}<span className="c-working tnum">{working} working</span></> : null}
              {finished > 0 ? <>{" · "}<span className="faint tnum">{finished} finished</span></> : null}
            </span>
          </button>
          {open ? (
            <Segmented<SubagentFilter>
              label="Filter subagents"
              value={effectiveFilter}
              onChange={onFilterChange}
              options={FILTERS}
            />
          ) : null}
        </div>
        {open ? (
          <div id={panelId} className="substrip-list" data-testid="subagent-strip-list">
            {visible.length ? (
              visible.map((s) => (
                <SubagentRow key={s.call_id} sessionKey={sessionKey} host={host} id={id} s={s} selected={s.call_id === selected} caps={snap.capabilities.subagents} />
              ))
            ) : (
              <div className="substrip-empty faint" data-testid="subagent-strip-empty">
                {effectiveFilter === "active" ? "No active subagents." : "No finished subagents."}
              </div>
            )}
          </div>
        ) : null}
      </div>
    </section>
  );
}
