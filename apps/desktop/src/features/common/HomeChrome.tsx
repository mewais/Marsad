import { useEffect, useRef } from "react";
import { useStore } from "../../lib/store";
import { useAppState, useCounts, useActions, usePending, useReach, type Counts } from "../../state";
import { Button, Icon, SearchField, Select, type SelectOption } from "../../ui";
import { PageHeader } from "../../shell/PageHeader";
import { clockTime } from "../../lib/format";
import { filterStore, filtersActive, resetFilters, setFilters } from "./filters";
import { STATUS, STATUS_ORDER } from "../../ui";

/** Live sessions only: sessions whose host is not reporting are counted apart, as last known. */
export function HomeSummary({ counts }: { counts: Counts }) {
  const lookedAt = useAppState((s) => s.lookedAt);
  const ready = useAppState((s) => s.fleetReady);
  // Nothing has been reported yet: zeros would claim there is nothing to see.
  if (!ready) return <span className="muted">Waiting for the first report from the local service…</span>;
  // No marker yet (or markers not loaded): there is no time to say.
  const since = lookedAt > 0 ? ` since you looked at ${clockTime(lookedAt)}` : "";
  return (
    <>
      <span className="c-needs"><Icon name="needs" size="sm" /> <b>{counts.needs}</b> need you</span>
      <span className="c-failed"><Icon name="failed" size="sm" /> <b>{counts.failed}</b> failed</span>
      <span className="c-done"><Icon name="done" size="sm" /> <b>{counts.done}</b> finished{since}</span>
      <span className="c-working"><Icon name="working" size="sm" spin /> <b>{counts.working}</b> working</span>
      <span className="muted"><Icon name="idle" size="sm" /> {counts.idle} idle</span>
      {counts.stale ? <span className="lk" data-testid="summary-stale" title="Their hosts are not reporting, so they are not counted above"><Icon name="clock" size="sm" /> <b>{counts.stale}</b> last known</span> : null}
    </>
  );
}

export function HomeHeader() {
  const counts = useCounts();
  const actions = useActions();
  const blocked = useReach();
  const marking = !!usePending("seen:all");
  return (
    <PageHeader
      title="Inbox"
      subtitle="All sessions, including those in workstreams"
      summary={<HomeSummary counts={counts} />}
      actions={
        <Button size="sm" variant="ghost" icon="eye" blocked={blocked} pending={marking} onClick={() => actions.markAllSeen()}>Mark all seen</Button>
      }
    />
  );
}

export const FILTER_INPUT_ID = "home-filter";

export function HomeToolbar() {
  const f = useStore(filterStore, (s) => s);
  const hostsMap = useAppState((s) => s.hosts);
  const order = useAppState((s) => s.hostOrder);
  const drivers = useAppState((s) => s.drivers);
  const driverOrder = useAppState((s) => s.driverOrder);
  const sessions = useAppState((s) => s.sessions);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const focus = () => input.current?.focus();
    window.addEventListener("marsad:focus-filter", focus);
    return () => window.removeEventListener("marsad:focus-filter", focus);
  }, []);
  const wsAll = useAppState((s) => s.workstreams);
  const ws = Object.values(wsAll).filter((w) => w.workstream.state !== "archived").map((w) => w.workstream.name);
  void sessions;
  const opt = (value: string, label: string): SelectOption => ({ value, label });
  return (
    <div className="toolbar" role="search">
      <SearchField ref={input} id={FILTER_INPUT_ID} width={280} placeholder="Filter by name, project, branch, error" aria-label="Filter sessions" kbd="/" value={f.q} onChange={(e) => setFilters({ q: e.target.value })}
        onKeyDown={(e) => { if (e.key === "Escape") { setFilters({ q: "" }); (e.target as HTMLElement).blur(); } }} />
      <Select label="Status" buttonClass={`chip ${f.status !== "all" ? "on" : ""}`} display={`Status: ${f.status === "all" ? "all" : STATUS[f.status].label.toLowerCase()}`} value={f.status}
        options={[opt("all", "All"), ...STATUS_ORDER.map((s) => opt(s, STATUS[s].label))]} onChange={(v) => setFilters({ status: v as typeof f.status })} />
      <Select label="Driver" buttonClass={`chip ${f.driver !== "any" ? "on" : ""}`} display={`Driver: ${f.driver === "any" ? "any" : drivers[f.driver]?.displayName ?? f.driver}`} value={f.driver}
        options={[opt("any", "Any"), ...driverOrder.map((d) => opt(d, drivers[d]?.displayName ?? d))]} onChange={(v) => setFilters({ driver: v })} />
      <Select label="Host" buttonClass={`chip ${f.hosts !== "all" ? "on" : ""}`} display={f.hosts === "all" ? `Host: ${order.length} of ${order.length}` : `Host: ${hostsMap[f.hosts[0]]?.entry.name ?? f.hosts[0]}`} value={f.hosts === "all" ? "all" : f.hosts[0]}
        options={[opt("all", "All hosts"), ...order.map((h) => opt(h, hostsMap[h]?.entry.name ?? h))]} onChange={(v) => setFilters({ hosts: v === "all" ? "all" : [v] })} />
      <Select label="Workstream" buttonClass={`chip hide-n ${f.workstream !== "any" ? "on" : ""}`} display={`Workstream: ${f.workstream === "any" ? "any" : f.workstream}`} value={f.workstream}
        options={[opt("any", "Any"), ...ws.map((w) => opt(w, w))]} onChange={(v) => setFilters({ workstream: v })} />
      {filtersActive(f) ? <button type="button" className="chip" onClick={resetFilters}><Icon name="x" size="xs" /> Clear filters</button> : null}
      <span className="sp" />
      <span className="chip" title="Sessions are sorted by what needs you, then by latest activity"><Icon name="sort" size="xs" /> Sort: attention</span>
    </div>
  );
}
