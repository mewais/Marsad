import type { ConfigItemKind, EffectiveItem, ItemValue, LayerScope } from "../../bridge/wire";
import type { IconName } from "../../ui";

/** How the effective config's provenance reads in the UI (pure; shared by the Config page and the session tab). */

export const scopeWord = (s: LayerScope): string => (s.scope === "global" ? "Global" : s.scope === "host" ? "Host" : s.scope === "workstream_node" ? "Workstream node" : "Session");
export const scopeCls = (s: LayerScope): string => (s.scope === "global" ? "g" : s.scope === "host" ? "h" : s.scope === "workstream_node" ? "w" : "s");

export interface Badge {
  cls: string;
  text: string;
}

export function provenance(it: EffectiveItem, driverName: string): Badge[] {
  const out: Badge[] = [];
  const o = it.origin;
  if (o.from === "agent") out.push({ cls: "a", text: `from ${driverName} config` });
  else if (o.from === "automatic") out.push({ cls: "wasl", text: `automatic · ${o.provider}` });
  else out.push({ cls: scopeCls(o.scope), text: `added at ${scopeWord(o.scope)}` });
  const c = it.changed_by;
  if (c && c.from === "layer") {
    if (it.state === "disabled") out.push({ cls: "x", text: `turned off at ${scopeWord(c.scope)}` });
    else if (it.state === "changed") out.push({ cls: scopeCls(c.scope), text: `changed at ${scopeWord(c.scope)}` });
    else out.push({ cls: "g", text: `reset at ${scopeWord(c.scope)}` });
  }
  return out;
}

export const KIND_ORDER: ConfigItemKind[] = ["mcp_server", "skill", "plugin", "command", "agent"];
export const KIND: Record<ConfigItemKind, { label: string; icon: IconName; one: string }> = {
  mcp_server: { label: "MCP servers", icon: "plug", one: "MCP server" },
  skill: { label: "Skills", icon: "book", one: "skill" },
  plugin: { label: "Plugins", icon: "puzzle", one: "plugin" },
  command: { label: "Slash commands", icon: "slash", one: "slash command" },
  agent: { label: "Custom agents", icon: "sparkle", one: "custom agent" },
};
/** Item kinds an override can set (add / change); the rest can only be switched off (marsad-proto `ItemValue`). */
export const SETTABLE: ConfigItemKind[] = ["mcp_server", "skill", "plugin"];

export function describeValue(v: ItemValue | null | undefined): string {
  if (!v) return "";
  if (v.kind === "mcp_server") return v.server.transport === "stdio" ? `stdio · ${[v.server.command, ...v.server.args].join(" ")}` : `HTTP · ${v.server.url}`;
  if (v.kind === "skill") return `skill root ${v.root}`;
  return `plugin at ${v.path}`;
}

export function describeItem(it: EffectiveItem): string {
  if (it.value) return describeValue(it.value);
  const b = it.base;
  if (!b) return "";
  if (b.mcp) return b.mcp.transport === "stdio" ? `stdio · ${[b.mcp.command, ...b.mcp.args].join(" ")}` : `HTTP · ${b.mcp.url}`;
  return b.description ?? b.path ?? "";
}

export const sourceOf = (it: EffectiveItem): string =>
  it.origin.from === "agent" ? it.origin.source.path ?? it.origin.source.scope : it.origin.from === "automatic" ? "Marsad" : `Marsad · ${scopeWord(it.origin.scope)}`;
