import { memo, useEffect, useState } from "react";
import { hostPath, useNavigate } from "../../router";
import { PageHeader } from "../../shell/PageHeader";
import { useShortcut } from "../../shortcuts/useShortcut";
import { driverVersionState, linkView, osWord, useActions, useApi, useAppState, useHost, useHosts, useReach } from "../../state";
import { Button, ConnState, EmptyState, Icon, Skeleton, hostIcon, useNow } from "../../ui";
import { AddHostDialog } from "./AddHostDialog";
import { HostDetail } from "./HostDetail";
import { SignInButton } from "./SignInButton";
import { linkSub } from "./linkText";
import "./hosts.css";

const HostRow = memo(function HostRow({ id, selected, onSelect }: { id: string; selected: boolean; onSelect: (id: string) => void }) {
  const h = useHost(id);
  const n = useAppState((s) => Object.keys(s.sessions).filter((k) => k.startsWith(id + "/")).length);
  const inv = useAppState((s) => s.inventory[id]);
  const drivers = useAppState((s) => s.drivers);
  const now = useNow(!!h && h.link.link === "remote" && h.link.state.state === "reconnecting");
  const arch = useAppStateArch(id);
  if (!h) return null;
  const lv = linkView(h.link, h.authPrompt);
  const connected = lv.connected;
  const installed = inv ? inv.filter((d) => d.installed) : [];
  const loadingAny = !!inv && connected && inv.some((d) => driverVersionState(d, connected).kind === "loading");
  return (
    <div role="option" aria-selected={selected} tabIndex={selected ? 0 : -1} className={`hrow${selected ? " on" : ""}`} data-host={id} onClick={() => onSelect(id)}
      onKeyDown={(e) => {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          const sib = (e.key === "ArrowDown" ? e.currentTarget.nextElementSibling : e.currentTarget.previousElementSibling) as HTMLElement | null;
          sib?.click();
          sib?.focus();
        }
      }}>
      <span><Icon name={hostIcon(h)} size="lg" /></span>
      <span className="hn"><span className="hn-name" title={h.name}>{h.name}</span><small title={`${[osWord(h.os), arch].filter(Boolean).join(" · ") || "Platform not probed yet"} · ${h.local ? "Local" : h.where}`}>{[osWord(h.os), arch].filter(Boolean).join(" · ") || "Platform not probed yet"} · {h.local ? "Local" : h.where}</small></span>
      <span className="hst"><ConnState host={h} /><span className="faint sub" title={linkSub(h.link, now)}>{linkSub(h.link, now)}</span>{lv.held ? <SignInButton host={h} /> : null}</span>
      <span className="small"><b className="tnum">{n}</b> <span className="faint">session{n === 1 ? "" : "s"}</span></span>
      <span className="clis">
        {!inv ? (
          <span className="faint small" role={connected ? "status" : undefined}>{connected ? "loading…" : "—"}</span>
        ) : installed.length > 0 ? (
          installed.map((d) => {
            const vs = driverVersionState(d, connected);
            return (
              <span key={d.driver} className="tag" title={d.display_name}>
                <b className="mono">{drivers[d.driver]?.initials ?? d.driver.slice(0, 2).toUpperCase()}</b>{" "}
                {vs.kind === "ready" ? vs.version : vs.kind === "loading" ? "loading…" : "not reported"}
              </span>
            );
          })
        ) : loadingAny ? (
          <span className="faint small" role="status">loading…</span>
        ) : (
          <span className="faint small">—</span>
        )}
      </span>
    </div>
  );
});
function useAppStateArch(id: string) {
  return useAppState((s) => s.hosts[id]?.entry.platform?.arch ?? "");
}

/** Hosts: every machine with its link in words, sessions and installed CLIs; add, remove, connect. */
export function HostsPage({ q }: { q: Record<string, string> }) {
  const hosts = useHosts();
  const ready = useAppState((s) => s.fleetReady);
  const nav = useNavigate();
  const api = useApi();
  const actions = useActions();
  const [adding, setAdding] = useState(q.add === "1");
  const sel = q.h && hosts.some((h) => h.id === q.h) ? q.h : hosts.find((h) => !linkView(h.link, h.authPrompt).connected)?.id ?? hosts[0]?.id;
  const selected = hosts.find((h) => h.id === sel);
  useEffect(() => setAdding(q.add === "1"), [q.add]);
  useShortcut({ id: "hosts-add", keys: ["a"], title: "Add host", group: "Hosts", run: () => setAdding(true) });
  const reach = useReach();
  const up = hosts.filter((h) => linkView(h.link, h.authPrompt).connected).length;
  const checkAll = () => {
    for (const h of hosts) {
      const l = linkView(h.link, h.authPrompt);
      if (l.connected) void actions.run(`refresh:${h.id}`, `Could not refresh ${h.name}`, () => api.refreshDrivers(h.id));
      else if (h.link.link === "remote" && h.link.state.state === "reconnecting") void actions.reconnectHost(h.id, h.name);
    }
  };
  return (
    <>
      <PageHeader title="Hosts" summary={<span className="muted">{ready ? `${hosts.length} machine${hosts.length === 1 ? "" : "s"} · ${up} connected` : "Waiting for the first report from the local service…"}</span>}
        actions={<><Button size="sm" icon="refresh" blocked={reach} onClick={checkAll}>Check all</Button><Button size="sm" variant="primary" icon="plus" kbd="A" onClick={() => setAdding(true)}>Add host</Button></>} />
      <div className="hs">
        <div className="hlist" role="listbox" aria-label="Hosts">
          {!ready ? <><Skeleton height={52} /><Skeleton height={52} /></> : hosts.length === 0 ? <EmptyState icon="server" title="No hosts yet" action={<Button size="sm" variant="primary" onClick={() => setAdding(true)}>Add host</Button>}>Add a machine over SSH or WSL.</EmptyState> : null}
          {hosts.map((h) => <HostRow key={h.id} id={h.id} selected={h.id === sel} onSelect={(id) => nav(hostPath(id))} />)}
        </div>
        {selected ? <HostDetail host={selected} /> : <aside className="hd" />}
      </div>
      <AddHostDialog open={adding} onClose={() => { setAdding(false); if (q.add) nav("/hosts"); }} />
    </>
  );
}
