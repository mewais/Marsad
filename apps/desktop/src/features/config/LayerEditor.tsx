import { useEffect, useState } from "react";
import type { ConfigItemKind, ItemValue, OverrideLayer } from "../../bridge/wire";
import { Button, Dialog, Icon, Select, TextField } from "../../ui";
import { withOverride, withoutOverride } from "./layers";
import { describeValue, KIND, SETTABLE } from "./provenance";

/** Edit one override layer: its item overrides, added items (MCP server, skill, plugin), env, instructions, tool limits. */
export function LayerEditor({ open, title, layer, driver, onSave, onClose, saving }: { open: boolean; title: string; layer: OverrideLayer; driver: string; onSave: (l: OverrideLayer) => void; onClose: () => void; saving: boolean }) {
  const [l, setL] = useState(layer);
  const [kind, setKind] = useState<ConfigItemKind>("mcp_server");
  const [name, setName] = useState("");
  const [transport, setTransport] = useState<"stdio" | "http">("stdio");
  const [cmd, setCmd] = useState("");
  const [env, setEnv] = useState("");
  const [onlyDriver, setOnlyDriver] = useState(false);
  useEffect(() => {
    if (open) setL(layer);
  }, [open, layer]);
  const value = (): ItemValue | null => {
    if (!cmd.trim()) return null;
    if (kind === "mcp_server") {
      if (transport === "http") return { kind: "mcp_server", server: { transport: "http", url: cmd.trim(), headers: {} } };
      const [command, ...args] = cmd.trim().split(/\s+/);
      return { kind: "mcp_server", server: { transport: "stdio", command, args, env: {} } };
    }
    return kind === "skill" ? { kind: "skill", root: cmd.trim() } : { kind: "plugin", path: cmd.trim() };
  };
  const add = () => {
    const v = value();
    if (!name.trim() || !v) return;
    setL((x) => withOverride(x, { kind, name: name.trim(), action: "set", value: v, driver: onlyDriver ? driver : null }));
    setName("");
    setCmd("");
  };
  const addEnv = () => {
    const m = /^([A-Za-z_][A-Za-z0-9_]*)=(.*)$/.exec(env.trim());
    if (!m) return;
    setL((x) => ({ ...x, env: [...x.env.filter(([k]) => k !== m[1]), [m[1], m[2]]] }));
    setEnv("");
  };
  const placeholder = kind === "mcp_server" ? (transport === "http" ? "https://mcp.example.com/sse" : "npx @playwright/mcp") : kind === "skill" ? "~/.marsad/skills (folder that contains <name>/SKILL.md)" : "~/plugins/my-plugin";
  return (
    <Dialog open={open} onClose={onClose} title={title} width={640}
      footer={<><span className="faint small">Applies only to sessions Marsad launches. The agent's own files are never changed.</span><span className="sp" /><Button onClick={onClose}>Cancel</Button><Button variant="primary" pending={saving} onClick={() => onSave(l)}>Save layer</Button></>}>
      <div className="led">
        <div className="field"><span className="lbl">Overrides in this layer</span>
          {l.items.length ? (
            <ul className="led-list">
              {l.items.map((o) => (
                <li key={`${o.kind}:${o.name}`}>
                  <Icon name={KIND[o.kind].icon} size="sm" /><b>{o.name}</b>
                  <span className="faint">{o.action === "disable" ? "turned off" : o.action === "reset" ? "reset to the agent's own" : describeValue(o.value)}{o.driver ? ` · only ${o.driver}` : ""}</span>
                  <span className="sp" /><button type="button" className="iconbtn" aria-label={`Remove override for ${o.name}`} onClick={() => setL((x) => withoutOverride(x, o.kind, o.name))}><Icon name="x" size="sm" /></button>
                </li>
              ))}
            </ul>
          ) : <span className="faint small">None yet. Switch items off in the table, or add one below.</span>}
        </div>
        <div className="field"><span className="lbl">Add or change an item</span>
          <div className="led-add">
            <Select label="Kind" value={kind} options={SETTABLE.map((k) => ({ value: k, label: KIND[k].one }))} onChange={(v) => setKind(v as ConfigItemKind)} />
            {kind === "mcp_server" ? <Select label="Transport" value={transport} options={[{ value: "stdio", label: "stdio" }, { value: "http", label: "HTTP" }]} onChange={(v) => setTransport(v as "stdio" | "http")} /> : null}
            <TextField aria-label="Item name" placeholder="name" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <TextField aria-label="Definition" placeholder={placeholder} value={cmd} onChange={(e) => setCmd(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
          <div className="row-gap"><label className="small chkl"><input type="checkbox" checked={onlyDriver} onChange={(e) => setOnlyDriver(e.target.checked)} /> Only for {driver}</label><span className="sp" /><Button size="sm" icon="plus" disabled={!name.trim() || !cmd.trim()} onClick={add}>Add to layer</Button></div>
          <span className="hint">Slash commands and custom agents can only be switched off (marsad overrides cannot define them).</span>
        </div>
        <div className="field"><span className="lbl">Environment</span>
          {l.env.map(([k, v]) => <div key={k} className="led-env mono">{k}={v}<button type="button" className="iconbtn" aria-label={`Remove ${k}`} onClick={() => setL((x) => ({ ...x, env: x.env.filter(([n]) => n !== k) }))}><Icon name="x" size="xs" /></button></div>)}
          <TextField aria-label="Environment variable" placeholder="NAME=value, Enter to add" value={env} onChange={(e) => setEnv(e.target.value)} onKeyDown={(e) => e.key === "Enter" && addEnv()} />
        </div>
        <div className="field"><label htmlFor="led-instr">Extra instructions</label>
          <textarea id="led-instr" className="ta" rows={3} value={l.extra_instructions ?? ""} onChange={(e) => setL((x) => ({ ...x, extra_instructions: e.target.value || null }))} placeholder="Appended to the agent's instructions (layers concatenate, broadest first)" />
        </div>
        <TextField label="Tools the agent must not use" hint="Comma-separated, the CLI's own tool names" value={l.disallowed_tools.join(", ")} onChange={(e) => setL((x) => ({ ...x, disallowed_tools: e.target.value.split(",").map((t) => t.trim()).filter(Boolean) }))} />
      </div>
    </Dialog>
  );
}
