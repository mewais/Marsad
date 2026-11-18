import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SessionKey } from "../../bridge/wire";
import { useStore } from "../../lib/store";
import { sessionPath, useNavigate } from "../../router";
import { useShortcut } from "../../shortcuts/useShortcut";
import { useActions, useAppState, useGroups, useApp, selectView } from "../../state";
import { Button, EmptyState, Icon, Kbd, STATUS, STATUS_ORDER, VirtualList, type StatusKey, type VirtualListHandle } from "../../ui";
import { filterStore, filtersActive, resetFilters } from "../common/filters";
import { HomeHeader, HomeToolbar } from "../common/HomeChrome";
import { InboxRow } from "./InboxRow";
import "./inbox.css";

type Entry = { type: "group"; status: StatusKey; count: number } | { type: "row"; key: SessionKey };

export function InboxPage() {
  const f = useStore(filterStore, (s) => s);
  const groups = useGroups(f);
  const nav = useNavigate();
  const actions = useActions();
  const { store } = useApp();
  const loaded = useAppState((s) => s.fleetReady);
  const list = useRef<VirtualListHandle>(null);
  const [selected, setSelected] = useState<SessionKey | null>(null);

  const rows = useMemo(() => STATUS_ORDER.flatMap((s) => groups[s]), [groups]);
  const entries = useMemo<Entry[]>(
    () => STATUS_ORDER.flatMap((s) => (groups[s].length ? ([{ type: "group", status: s, count: groups[s].length } as Entry, ...groups[s].map((key) => ({ type: "row", key }) as Entry)]) : [])),
    [groups],
  );
  // Keep a valid selection: default to the first row that needs you.
  useEffect(() => {
    if (!selected || !rows.includes(selected)) setSelected(groups.needs[0] ?? rows[0] ?? null);
  }, [rows, groups, selected]);

  const open = useCallback((key: SessionKey) => {
    const [host, ...rest] = key.split("/");
    nav(sessionPath(host, rest.join("/")));
  }, [nav]);
  const move = (d: 1 | -1) => {
    if (!rows.length) return;
    const i = selected ? rows.indexOf(selected) : -1;
    const next = rows[Math.min(rows.length - 1, Math.max(0, i + d))];
    setSelected(next);
    list.current?.scrollToKey(next);
  };
  const selView = selected ? selectView(store.get(), selected) : undefined;

  useShortcut({ id: "inbox-next", keys: ["j"], title: "Next session", group: "Inbox", run: () => move(1) });
  useShortcut({ id: "inbox-prev", keys: ["k"], title: "Previous session", group: "Inbox", run: () => move(-1) });
  useShortcut({ id: "inbox-down", keys: ["arrowdown"], title: "Next session", group: "Inbox", run: () => move(1) });
  useShortcut({ id: "inbox-up", keys: ["arrowup"], title: "Previous session", group: "Inbox", run: () => move(-1) });
  useShortcut({ id: "inbox-open", keys: ["enter"], title: "Open the selected session", group: "Inbox", when: () => !!selected, run: () => selected && open(selected) });
  useShortcut({ id: "inbox-seen", keys: ["e"], title: "Mark seen", group: "Inbox", when: () => selView?.status === "done", run: () => selected && actions.markSeen([selected]) });
  useShortcut({ id: "inbox-filter", keys: ["/"], title: "Filter", group: "Inbox", run: () => window.dispatchEvent(new Event("marsad:focus-filter")) });

  const loading = !loaded;
  return (
    <>
      <HomeHeader />
      <HomeToolbar />
      <div className="ibx-head" aria-hidden="true">
        <span>Status</span><span>Session</span><span className="drvh">Driver</span><span>Host</span><span>Project · branch</span><span className="r">Last</span><span className="r">Cost</span>
      </div>
      {entries.length === 0 ? (
        <div className="ibx">
          {loading ? <EmptyState icon="working" title="Connecting to your hosts">Sessions appear as each host answers.</EmptyState>
            : filtersActive(f) ? <EmptyState icon="filter" title="No session matches" action={<Button size="sm" onClick={resetFilters}>Clear filters</Button>}>Nothing fits the current filters.</EmptyState>
            : <EmptyState icon="inbox" title="No sessions yet">Start a session on any host and it shows up here.</EmptyState>}
        </div>
      ) : (
        <VirtualList<Entry>
          ref={list}
          className="ibx"
          role="list"
          aria-label="Sessions"
          items={entries}
          getKey={(e) => (e.type === "group" ? `g:${e.status}` : e.key)}
          estimateSize={(e) => (e.type === "group" ? 40 : 108)}
          renderItem={(e) =>
            e.type === "group" ? (
              <div className={`grp c-${e.status}`} role="presentation">
                <Icon name={STATUS[e.status].icon} size="sm" spin={e.status === "working"} />
                {STATUS[e.status].group}
                <span className="cnt">{e.count}</span>
                <span className="gline" />
                {e.status === "needs" ? <span className="gact">Answer here or press Enter to open</span> : null}
                {e.status === "done" ? <Button size="sm" variant="ghost" onClick={() => actions.markSeen(groups.done)}>Mark all seen</Button> : null}
              </div>
            ) : (
              <InboxRow sessionKey={e.key} selected={e.key === selected} onSelect={setSelected} onOpen={open} />
            )
          }
        />
      )}
      <div className="keyhints">
        <span><Kbd>J</Kbd><Kbd>K</Kbd> move</span>
        <span><Kbd>Enter</Kbd> open</span>
        <span><Kbd>1</Kbd>–<Kbd>3</Kbd> pick the agent's choice</span>
        <span><Kbd>A</Kbd> allow · <Kbd>D</Kbd> deny</span>
        <span><Kbd>E</Kbd> mark seen</span>
        <span><Kbd>/</Kbd> filter</span>
        <span><Kbd>G</Kbd> then <Kbd>W</Kbd> workstreams</span>
      </div>
    </>
  );
}
