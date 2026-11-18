import { useEffect, useMemo, useState } from "react";
import type { WorkstreamNode, WorkstreamTemplate } from "../../bridge/wire";
import { sessionKey, splitKey } from "../../bridge/wire";
import { useNavigate, workstreamPath } from "../../router";
import { selectView, useActions, useApi, useApp, useAppState, usePending, workstreamIndex } from "../../state";
import { Button, Dialog, DriverBadge, Icon, SearchField, Select, STATUS, TextField, makeDriver } from "../../ui";

type Start = "blank" | "template" | "sessions";

/** Node id from a display name (`[a-z0-9_-]`, unique in the graph). */
export function nodeId(name: string, taken: Set<string>): string {
  const base = name.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || "agent";
  let id = base;
  for (let i = 2; taken.has(id); i++) id = `${base}-${i}`;
  taken.add(id);
  return id;
}

/** Step 1 of 2: name, description and a starting point (blank, a template, or sessions already running). */
export function NewWorkstreamDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const { store } = useApp();
  const sessions = useAppState((s) => s.sessions);
  const wss = useAppState((s) => s.workstreams);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [start, setStart] = useState<Start>("blank");
  const [tpls, setTpls] = useState<WorkstreamTemplate[]>([]);
  const [tpl, setTpl] = useState("");
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const pending = !!usePending("ws.create");
  useEffect(() => {
    if (!open) return;
    api.ws.templates().then((t) => {
      setTpls(t.templates);
      setTpl((x) => x || t.templates[0]?.id || "");
    }, () => setTpls([]));
  }, [open, api]);

  // Sessions already in another workstream are not offered.
  const free = useMemo(() => {
    const inWs = workstreamIndex(wss);
    const n = q.trim().toLowerCase();
    return Object.keys(sessions).filter((k) => !inWs.has(k)).map((k) => selectView(store.get(), k)!).filter((v) => !n || `${v.name} ${v.project}`.toLowerCase().includes(n)).sort((a, b) => b.lastMs - a.lastMs);
  }, [sessions, wss, q, store]);
  const drivers = useAppState((s) => s.drivers);
  const hostsMap = useAppState((s) => s.hosts);

  const problem = !name.trim() ? "Give it a name." : start === "template" && !tpl ? "Pick a template." : start === "sessions" && !picked.size ? "Pick at least one session." : "";
  const submit = async () => {
    if (problem) return;
    const w = await actions.run("ws.create", "Could not create the workstream", async () => {
      if (start === "template") {
        const made = await api.ws.fromTemplate({ template: tpl, name: name.trim() });
        return desc.trim() ? api.ws.update({ ...made, description: desc.trim() }, made.revision) : made;
      }
      const taken = new Set<string>();
      const nodes: WorkstreamNode[] = start === "sessions" ? [...picked].map((k) => {
        const { host, id } = splitKey(k);
        const nm = selectView(store.get(), k)?.name ?? id;
        return { id: nodeId(nm, taken), name: nm, role: "", description: "", session: { host, session: id }, launch: null };
      }) : [];
      return api.ws.create({ name: name.trim(), description: desc.trim(), id: null, nodes, edges: [], limits: { max_depth: 4, default_deadline_secs: 1800, budget_usd: null }, layout: {} });
    });
    if (!w) return;
    onClose();
    nav(workstreamPath(w.id));
  };
  const toggle = (k: string) => setPicked((s) => {
    const n = new Set(s);
    if (n.has(k)) n.delete(k);
    else n.add(k);
    return n;
  });
  const opt = (v: Start, icon: "plus" | "book" | "refresh", title: string, d: string, extra?: React.ReactNode) => (
    <div className={`po${start === v ? " on" : ""}`}>
      <label className="po-l"><input type="radio" name="ws-start" checked={start === v} onChange={() => setStart(v)} /><Icon name={icon} size="sm" /><span><b>{title}</b><span className="d">{d}</span></span></label>
      {start === v ? extra : null}
    </div>
  );

  return (
    <Dialog open={open} onClose={onClose} title="New workstream" width={600}
      headerExtra={<div className="steps"><span className="s on"><b>1</b>Name and start</span><Icon name="chevr" size="xs" /><span className="s"><b>2</b>Design</span></div>}
      footer={<><span className="faint small">{problem || "Nothing starts until you launch from the design."}</span><span className="sp" /><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={!!problem} pending={pending} onClick={() => void submit()}>Continue to design <Icon name="chevr" size="xs" /></Button></>}>
      <div className="nwd">
        <TextField label="Name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Docs search revamp" />
        <div className="field"><label htmlFor="wsd">Description <span className="faint optl">optional</span></label><textarea id="wsd" className="ta" rows={2} value={desc} onChange={(e) => setDesc(e.target.value)} /></div>
        <div className="field"><span className="lbl">Start from</span>
          <div className="pol startopts" role="radiogroup" aria-label="Start from">
            {opt("blank", "plus", "Blank", "An empty graph with a lane for each connected host.")}
            {opt("template", "book", "A saved template", tpls.map((t) => t.name).join(" · ") || "No templates saved yet", tpls.length ? <div className="po-x"><Select label="Template" value={tpl || undefined} options={tpls.map((t) => ({ value: t.id, label: t.name, description: t.description ?? "" }))} onChange={setTpl} /></div> : null)}
            {opt("sessions", "refresh", "From existing sessions", "Pull in sessions that are already running or paused. They keep their history.", (
              <div className="po-x">
                <SearchField placeholder="Search your sessions" aria-label="Search sessions" value={q} onChange={(e) => setQ(e.target.value)} />
                <div className="hostpick pick-sessions" role="group" aria-label="Sessions to pull in">
                  {free.slice(0, 40).map((v) => (
                    <label key={v.key} className={`hp${picked.has(v.key) ? " on" : ""}`}>
                      <input type="checkbox" checked={picked.has(v.key)} onChange={() => toggle(v.key)} />
                      <DriverBadge driver={drivers[v.driver] ?? makeDriver(v.driver)} compact /><b>{v.name}</b><span className="faint">{hostsMap[v.host]?.entry.name ?? v.host}</span><span className="sp" /><span className={`c-${v.status} small`}>{STATUS[v.status].label}</span>
                    </label>
                  ))}
                  {free.length === 0 ? <span className="faint small hp">No sessions match.</span> : null}
                </div>
                <span className="faint small">{picked.size} selected. Sessions already in another workstream are not listed.</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </Dialog>
  );
}

export const keyOfNode = (host: string, session: string) => sessionKey(host, session);
