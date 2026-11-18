import type { ConfigItem, EffectiveConfig, EffectiveItem, IgnoredOverride, ItemOverride, LayerScope, OverrideLayer, StoredLayer, Unsupported } from "../wire";

/**
 * Fixture-only: Marsad's stored override layers and a small re-implementation of the hub's
 * `effective_config` merge (marsad-proto agentconfig), so the UI can be exercised end to end.
 */

const emptyLayer = (): OverrideLayer => ({ items: [], env: [], disallowed_tools: [] });

export function seedLayers(): StoredLayer[] {
  return [
    { scope: { scope: "global" }, layer: { items: [{ kind: "mcp_server", name: "linear", action: "set", value: { kind: "mcp_server", server: { transport: "http", url: "https://mcp.linear.app/sse", headers: {} } } }], extra_instructions: "Follow the conventions in AGENTS.md.", env: [], disallowed_tools: [] } },
    { scope: { scope: "host", host: "x1" }, layer: { items: [
      { kind: "mcp_server", name: "sentry", action: "disable" },
      { kind: "mcp_server", name: "postgres-ledgerline", action: "set", value: { kind: "mcp_server", server: { transport: "stdio", command: "postgres-mcp", args: ["postgres://localhost:5432/ledgerline_dev"], env: {} } } },
    ], env: [["PGHOST", "localhost"]], disallowed_tools: [] } },
    { scope: { scope: "workstream_node", workstream: "ws-invoice", node: "orchestrator" }, layer: { items: [
      { kind: "mcp_server", name: "playwright", action: "set", value: { kind: "mcp_server", server: { transport: "stdio", command: "npx", args: ["@playwright/mcp"], env: {} } } },
      { kind: "skill", name: "pdf-tools", action: "set", value: { kind: "skill", root: "~/.marsad/skills" } },
    ], env: [], disallowed_tools: [] } },
  ];
}

export const sameScope = (a: LayerScope, b: LayerScope) => JSON.stringify(a) === JSON.stringify(b);
export const layerAt = (layers: StoredLayer[], scope: LayerScope): OverrideLayer => layers.find((l) => sameScope(l.scope, scope))?.layer ?? emptyLayer();

const UNSUPPORTED: Record<string, Partial<Record<string, string>>> = {
  kilo: { plugin: "Kilo Code has no plugin hooks. Use a git pre-commit hook instead.", skill: "Kilo Code reads rules, not skills." },
  codex: { plugin: "Codex has no plugin loader." },
};

export interface MergeInput {
  driver: string;
  base: ConfigItem[];
  layers: { scope: LayerScope; layer: OverrideLayer }[];
  /** Workstream sessions get Marsad's orchestration server. */
  workstream: boolean;
}

export function mergeEffective({ driver, base, layers, workstream }: MergeInput): EffectiveConfig {
  const items = new Map<string, EffectiveItem>();
  const k = (kind: string, name: string) => `${kind}:${name}`;
  for (const b of base) items.set(k(b.kind, b.name), { kind: b.kind, name: b.name, state: "inherited", active: b.enabled, origin: { from: "agent", source: b.source }, changed_by: null, base: b, value: null, locked: false, unsupported: null });
  const ignored: IgnoredOverride[] = [];
  const env = new Map<string, [string, LayerScope]>();
  const disallowed = new Set<string>();
  const instr: string[] = [];
  const instrScopes: LayerScope[] = [];
  for (const { scope, layer } of layers) {
    for (const o of layer.items as ItemOverride[]) {
      if (o.driver && o.driver !== driver) continue;
      const key = k(o.kind, o.name);
      const cur = items.get(key);
      const by = { from: "layer" as const, scope };
      if (o.action === "disable") {
        if (!cur) ignored.push({ scope, item: { kind: o.kind, name: o.name }, reason: "The agent has no item by that name." });
        else items.set(key, { ...cur, state: "disabled", active: false, changed_by: by });
      } else if (o.action === "set") {
        if (o.kind === "command" || o.kind === "agent") {
          ignored.push({ scope, item: { kind: o.kind, name: o.name }, reason: "Marsad can only switch slash commands and custom agents off." });
        } else if (cur?.base) items.set(key, { ...cur, state: "changed", active: true, value: o.value, changed_by: by });
        else items.set(key, { kind: o.kind, name: o.name, state: "added", active: true, origin: by, changed_by: null, base: null, value: o.value, locked: false, unsupported: null });
      } else if (cur) {
        if (cur.base) items.set(key, { ...cur, state: "inherited", active: cur.base.enabled, value: null, changed_by: by });
        else items.delete(key);
      }
    }
    for (const [name, value] of layer.env) env.set(name, [value, scope]);
    layer.disallowed_tools.forEach((t) => disallowed.add(t));
    if (layer.extra_instructions) {
      instr.push(layer.extra_instructions);
      instrScopes.push(scope);
    }
  }
  if (workstream) {
    items.set(k("mcp_server", "wasl"), {
      kind: "mcp_server", name: "wasl", state: "automatic", active: true, origin: { from: "automatic", provider: "Wasl" }, changed_by: null, base: null,
      value: { kind: "mcp_server", server: { transport: "stdio", command: "marsad-agent", args: ["wasl-mcp"], env: {} } }, locked: true, unsupported: null,
    });
  }
  const unsupported: Unsupported[] = [];
  const out = [...items.values()].map((it) => {
    const why = UNSUPPORTED[driver]?.[it.kind];
    return why && it.active ? { ...it, unsupported: why } : it;
  });
  if (driver === "kilo" && instr.length) unsupported.push({ item: "extra_instructions", reason: "Kilo Code reads instructions only from .kilo/rules; add them there." });
  out.sort((a, b) => a.kind.localeCompare(b.kind) || a.name.localeCompare(b.name));
  return {
    driver,
    items: out,
    extra_instructions: instr.length ? { value: instr.join("\n\n"), scopes: instrScopes } : null,
    env: [...env.entries()].map(([n, [v, s]]) => [n, v, s] as [string, string, LayerScope]),
    disallowed_tools: [...disallowed],
    ignored,
    unsupported,
  };
}
