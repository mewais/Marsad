import { useEffect, useState } from "react";
import { errorMessage } from "../../bridge/api";
import type { WorkstreamTemplate } from "../../bridge/wire";
import { agoLong, money } from "../../lib/format";
import { useNavigate, workstreamPath } from "../../router";
import { openNewWorkstream } from "../../shell/dialogs";
import { PageHeader } from "../../shell/PageHeader";
import { useActions, useApi, useAppState, useHost } from "../../state";
import { Button, EmptyState, HostChip, Icon, Skeleton } from "../../ui";
import { summary } from "./model";
import "./workstreams.css";

function HostCell({ id }: { id: string }) {
  const h = useHost(id);
  return h ? <HostChip host={h} /> : <span className="faint">{id}</span>;
}

function Row({ id }: { id: string }) {
  const v = useAppState((s) => s.workstreams[id]);
  const nav = useNavigate();
  if (!v) return null;
  const w = v.workstream;
  const s = summary(v);
  const go = () => nav(workstreamPath(w.id));
  return (
    <div className="wsrow" role="link" tabIndex={0} data-ws={w.id} onClick={go} onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), go())} aria-label={`${w.name}: ${s.parts.map((p) => p.text).join(", ")}`}>
      <span className="nm"><b><Icon name="graph" size="sm" className={w.state === "draft" ? "c-accent" : ""} />{w.name}</b><span>{w.description}</span></span>
      <span className="hm">{s.hosts.map((h) => <HostCell key={h} id={h} />)}</span>
      <span className="tnum">{w.state === "draft" ? `${w.nodes.length} planned` : s.sessions}</span>
      <span className="sw">{s.parts.map((p) => <span key={p.text} className={`c-${p.cls} swp`}><Icon name={p.icon} size="sm" spin={p.icon === "working"} />{p.text}</span>)}</span>
      <span className="cost r tnum">{money(s.cost)}</span>
      <span className="ago">{w.state === "draft" ? "edited " : "updated "}{agoLong(w.updated_at_ms)}</span>
      <span className="faint"><Icon name="chevr" size="sm" /></span>
    </div>
  );
}

/** Workstreams: every graph with hosts, sessions, status in words and cost; templates to start from. */
export function WorkstreamsPage() {
  const order = useAppState((s) => s.wsOrder);
  const loaded = useAppState((s) => s.workstreamsLoaded);
  const archived = useAppState((s) => s.wsOrder.filter((id) => s.workstreams[id]?.workstream.state === "archived").join("|"));
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const [tpls, setTpls] = useState<WorkstreamTemplate[] | null>(null);
  const [tplErr, setTplErr] = useState("");
  useEffect(() => {
    api.ws.templates().then((t) => setTpls(t.templates), (e) => setTplErr(errorMessage(e)));
  }, [api]);
  const fromTemplate = async (t: WorkstreamTemplate) => {
    const w = await actions.run(`tpl:${t.id}`, "Could not start from the template", () => api.ws.fromTemplate({ template: t.id }));
    if (w) nav(workstreamPath(w.id));
  };
  const active = order.filter((id) => !archived.split("|").includes(id));
  return (
    <>
      <PageHeader title="Workstreams" subtitle="Agents that hand work to each other. Their sessions also show in your Inbox."
        actions={<Button size="sm" variant="primary" icon="plus" kbd="⇧N" onClick={() => openNewWorkstream()}>New workstream</Button>} />
      <div className="wsl">
        <div className="wsh" aria-hidden="true"><span>Name</span><span>Hosts</span><span>Sessions</span><span>Status</span><span className="r">Cost</span><span>Last</span><span /></div>
        {!loaded ? <><Skeleton height={56} /><Skeleton height={56} /></> : active.length === 0 ? (
          <EmptyState icon="graph" title="No workstreams yet" action={<Button size="sm" variant="primary" onClick={() => openNewWorkstream()}>New workstream</Button>}>A workstream lets agents on different hosts hand work to each other, with you approving handoffs.</EmptyState>
        ) : <div role="list" aria-label="Workstreams" className="wslist">{active.map((id) => <div role="listitem" key={id}><Row id={id} /></div>)}</div>}
        <div className="eyebrow wst-h">Saved templates</div>
        <div className="tpl">
          {tplErr ? <span className="c-failed small">{tplErr}</span> : !tpls ? <Skeleton height={60} /> : tpls.length === 0 ? <span className="faint small">No templates yet. Save a workstream as a template from its menu.</span> : tpls.map((t) => (
            <button key={t.id} type="button" className="card tplc" onClick={() => void fromTemplate(t)} title="Start a new draft from this template">
              <b>{t.name}</b><span className="faint small">{t.description || `${t.nodes.length} agents`}</span>
            </button>
          ))}
        </div>
        {archived ? <><div className="eyebrow wst-h">Archived</div><div role="list" aria-label="Archived workstreams" className="wslist">{archived.split("|").map((id) => <div role="listitem" key={id}><Row id={id} /></div>)}</div></> : null}
      </div>
    </>
  );
}
