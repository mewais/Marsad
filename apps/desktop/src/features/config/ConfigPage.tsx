import { useCallback, useEffect, useMemo, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { BaseConfig, ConfigItemKind, EffectiveConfig, EffectiveItem, LayerScope, StoredLayer } from "../../bridge/wire";
import { sessionKey } from "../../bridge/wire";
import { useNavigate } from "../../router";
import { PageHeader } from "../../shell/PageHeader";
import { linkView, sessionName, useActions, useApi, useAppState, useHosts, usePending } from "../../state";
import { Button, DriverBadge, EmptyState, Icon, Segmented, Select, Skeleton, makeDriver, toast } from "../../ui";
import { EffectiveTable } from "./EffectiveTable";
import { LayerEditor } from "./LayerEditor";
import { counts, layerOf, sources, summarize, toggled, withoutOverride } from "./layers";
import { KIND } from "./provenance";
import "./config.css";

interface LayerDef {
  key: string;
  label: string;
  where: string;
  icon: "term" | "graph" | "laptop" | "layers";
  scope: LayerScope | null;
  why?: string;
}

/**
 * Agent config: what each CLI already has (read-only base, from `agentconfig.base`), Marsad's
 * override layers Global → Host → Workstream node → Session (`store.layers.*`), and the effective
 * result for one launch context with provenance (`agentconfig.effective`).
 */
export function ConfigPage({ q }: { q: Record<string, string> }) {
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const hosts = useHosts();
  const drivers = useAppState((s) => s.drivers);
  const order = useAppState((s) => s.driverOrder);
  const sessions = useAppState((s) => s.sessions);
  const wss = useAppState((s) => s.workstreams);
  const driver = q.driver && drivers[q.driver] ? q.driver : order[0] ?? "";
  const upHosts = hosts.filter((h) => linkView(h.link, h.authPrompt).connected);
  const host = q.host && hosts.some((h) => h.id === q.host) ? q.host : upHosts[0]?.id ?? hosts[0]?.id ?? "";
  const session = q.session && sessions[sessionKey(host, q.session)]?.driver === driver ? q.session : "";
  const snap = session ? sessions[sessionKey(host, session)] : undefined;
  const set = (p: Record<string, string>) => nav(`/config?${new URLSearchParams(Object.fromEntries(Object.entries({ driver, host, session, ...p }).filter(([, v]) => v)))}`);

  const wsNode = useMemo(() => {
    if (!session) return null;
    for (const v of Object.values(wss)) {
      const n = v.workstream.nodes.find((x) => x.session?.host === host && x.session.session === session);
      if (n && v.workstream.state !== "archived") return { workstream: v.workstream.id, node: n.id, label: `${n.name} · ${v.workstream.name}` };
    }
    return null;
  }, [wss, host, session]);

  const [base, setBase] = useState<BaseConfig | null>(null);
  const [baseErr, setBaseErr] = useState("");
  const [layers, setLayers] = useState<StoredLayer[] | null>(null);
  const [eff, setEff] = useState<EffectiveConfig | null>(null);
  const [effErr, setEffErr] = useState("");
  const [filter, setFilter] = useState<ConfigItemKind | "all">("all");
  const [selKey, setSelKey] = useState("session");
  const [editing, setEditing] = useState(false);
  const project = snap?.cwd ?? null;

  const loadBase = useCallback((refresh: boolean) => {
    if (!driver || !host) return;
    setBaseErr("");
    if (refresh) setBase(null);
    api.config.base(host, { driver, project, refresh }).then(setBase, (e) => setBaseErr(errorMessage(e)));
  }, [api, driver, host, project]);
  const loadEff = useCallback(() => {
    if (!driver || !host) return;
    setEffErr("");
    api.config.effective({ host, driver, project, workstream: wsNode ? { workstream: wsNode.workstream, node: wsNode.node } : null, session: session || null }).then(setEff, (e) => setEffErr(errorMessage(e)));
  }, [api, driver, host, project, wsNode, session]);
  useEffect(() => loadBase(false), [loadBase]);
  useEffect(loadEff, [loadEff]);
  useEffect(() => {
    api.store.layers().then((l) => setLayers(l.layers), (e) => toast({ tone: "err", title: "Could not load your overrides", detail: errorMessage(e) }));
  }, [api]);

  const hostName = hosts.find((h) => h.id === host)?.name ?? host;
  const drvName = drivers[driver]?.displayName ?? driver;
  const defs: LayerDef[] = [
    { key: "session", label: "Session", where: snap ? `${sessionName(snap)} · this session only` : "pick a session to edit its layer", icon: "term", scope: session ? { scope: "session", host, session } : null },
    { key: "node", label: "Workstream node", where: wsNode ? wsNode.label : "none · standalone session", icon: "graph", scope: wsNode ? { scope: "workstream_node", workstream: wsNode.workstream, node: wsNode.node } : null, why: "Only sessions in a workstream have this layer." },
    { key: "host", label: "Host", where: `${hostName} · sessions launched on this host`, icon: "laptop", scope: host ? { scope: "host", host } : null },
    { key: "global", label: "Global", where: "all hosts · every session Marsad launches", icon: "layers", scope: { scope: "global" } },
  ];
  const sel = defs.find((d) => d.key === selKey && d.scope) ?? defs.find((d) => d.key === "host" && d.scope) ?? defs[3];
  const selLayer = layers ? layerOf(layers, sel.scope!) : null;
  const savePk = "layers.set";
  const saving = !!usePending(savePk);

  const save = async (layer: typeof selLayer) => {
    if (!layer || !sel.scope) return;
    const r = await actions.run(savePk, "Could not save the override", () => api.store.setLayer({ scope: sel.scope!, layer }));
    if (r === undefined) return;
    setLayers((all) => [...(all ?? []).filter((l) => JSON.stringify(l.scope) !== JSON.stringify(sel.scope)), { scope: sel.scope!, layer }]);
    setEditing(false);
    loadEff();
  };
  const onToggle = (it: EffectiveItem) => selLayer && sel.scope && void save(toggled(selLayer, it, sel.scope));
  const onUndo = (it: EffectiveItem) => selLayer && void save(withoutOverride(selLayer, it.kind, it.name));

  const nItems = eff?.items.length ?? 0;
  const nActive = eff?.items.filter((i) => i.active && !i.unsupported).length ?? 0;
  const nUns = eff?.items.filter((i) => i.unsupported).length ?? 0;
  const hostSessions = Object.entries(sessions).filter(([k, s]) => k.startsWith(host + "/") && s.driver === driver);

  if (!order.length) return <><PageHeader title="Agent config" /><div className="placeholder"><EmptyState icon="layers" title="No agents known yet">Connect a host with an agent CLI installed; its configuration shows here.</EmptyState></div></>;
  return (
    <>
      <PageHeader title="Agent config" subtitle="Marsad mirrors what each agent CLI already has. Overrides apply only to sessions launched from Marsad."
        actions={<><Button size="sm" icon="refresh" onClick={() => loadBase(true)}>Refresh from agents</Button><Button size="sm" variant="primary" icon="plus" disabled={!selLayer} onClick={() => setEditing(true)}>Add override</Button></>} />
      <div className="ac">
        <aside className="stack" aria-label="Configuration layers">
          <div className="ctxsel">
            <div className="seg drvseg" role="group" aria-label="Agent">
              {order.map((d) => <button key={d} type="button" aria-pressed={d === driver} title={drivers[d]?.displayName} onClick={() => set({ driver: d, session: "" })}><DriverBadge driver={drivers[d] ?? makeDriver(d)} compact /><span>{drivers[d]?.displayName}</span></button>)}
            </div>
          </div>
          <div className="eyebrow stack-cap">Read from the bottom up · higher layers win</div>
          {defs.map((d) => {
            const l = d.scope && layers ? layerOf(layers, d.scope) : null;
            const lines = l ? summarize(l, base?.items ?? []) : [];
            return (
              <div key={d.key}>
                <button type="button" className={`layer${sel.key === d.key ? " on" : ""}${d.scope ? "" : " na"}`} disabled={!d.scope} aria-pressed={sel.key === d.key} onClick={() => setSelKey(d.key)} title={d.scope ? `Edit the ${d.label} layer` : d.why}>
                  <span className="lh"><Icon name={d.icon} size="sm" />{d.label}<span className="sp" /><span className="lv">Marsad</span></span>
                  <span className="faint small">{d.where}</span>
                  {lines.map((x, i) => <span key={i} className="dl"><span className={`op ${x.cls}`}>{x.op}</span>{x.text}</span>)}
                  {d.scope && l && !lines.length ? <span className="dl faint"><span className="op">·</span>Nothing changed here</span> : null}
                  {d.key === "node" && wsNode ? <span className="dl"><span className="op c-accent"><Icon name="lock" size="xs" /></span>Wasl connector · automatic</span> : null}
                </button>
                <div className="arrowup" aria-hidden="true"><Icon name="arrowdown" size="sm" /></div>
              </div>
            );
          })}
          <div className="layer base">
            <span className="lh"><DriverBadge driver={drivers[driver] ?? makeDriver(driver)} compact />From the agent<span className="sp" /><span className="ro">read-only</span></span>
            {baseErr ? <span className="c-failed small">{baseErr}</span> : !base ? <Skeleton /> : (
              <>
                <span className="faint small">{drvName} on {hostName}{project ? ` · ${project}` : ""} · discovered {new Date(base.refreshed_at_ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}{base.refreshing ? " · still probing" : ""}</span>
                <span className="dl fg">{counts(base.items) || "Nothing configured"}</span>
                {base.errors.map((e) => <span key={e} className="dl c-failed">{e}</span>)}
                <span className="srcs">{sources(base.items).map((s) => <span key={s.path} className="mono" title={s.what}>{s.path}</span>)}</span>
              </>
            )}
          </div>
        </aside>
        <section className="eff" aria-label="Effective configuration">
          <div className="eff-h">
            <span className="h3">Effective for</span>
            <Select label="Host" value={host} options={hosts.map((h) => ({ value: h.id, label: h.name, description: linkView(h.link, h.authPrompt).word }))} onChange={(h) => set({ host: h, session: "" })} />
            <Select label="Session" value={session || "-"} options={[{ value: "-", label: "A new launch" }, ...hostSessions.map(([, s]) => ({ value: s.id, label: sessionName(s) }))]} onChange={(v) => set({ session: v === "-" ? "" : v })} />
            <Segmented label="Kind" value={filter} onChange={setFilter} options={[{ value: "all", label: "All" }, ...(["mcp_server", "skill", "plugin", "command", "agent"] as ConfigItemKind[]).map((k) => ({ value: k, label: k === "mcp_server" ? "MCP" : KIND[k].label.replace("Slash commands", "Commands").replace("Custom agents", "Agents") }))]} />
            <span className="sp" />
            <span className="faint small">{nItems} items · {nActive} active · {nUns} not applied</span>
          </div>
          <div className="eff-b">
            <div className="note"><Icon name="lock" size="sm" /><span>{drvName}'s own config files are never modified, and sessions you start in a terminal keep their normal setup. Switches below edit the <b>{sel.label}</b> layer. Marsad adds two things by itself: the Wasl connector (so an agent can delegate once a workstream connects it) and, for agents that cannot ask you questions themselves, the ask_user tool.</span></div>
            {effErr ? <div className="c-failed small" role="alert">{effErr}</div> : !eff ? <div className="fl-pad"><Skeleton /><Skeleton width="80%" /><Skeleton width="60%" /></div> : <EffectiveTable eff={eff} driverName={drvName} filter={filter} onToggle={onToggle} busy={saving || !selLayer} scope={sel.scope ?? null} onUndo={onUndo} />}
            {eff?.ignored.length ? <div className="note">{eff.ignored.map((i) => <span key={`${i.item.kind}${i.item.name}`}>{i.item.name}: {i.reason}</span>)}</div> : null}
            {eff?.unsupported.map((u) => <div key={u.item} className="note"><Icon name="minus" size="sm" /><span><b>{u.item}</b>: {u.reason}</span></div>)}
          </div>
        </section>
      </div>
      {selLayer ? <LayerEditor open={editing} title={`${sel.label} layer · ${sel.where}`} layer={selLayer} driver={driver} saving={saving} onSave={(l) => void save(l)} onClose={() => setEditing(false)} /> : null}
    </>
  );
}
