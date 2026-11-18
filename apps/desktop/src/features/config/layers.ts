import type { ConfigItem, ConfigItemKind, EffectiveItem, ItemOverride, LayerScope, OverrideLayer, StoredLayer } from "../../bridge/wire";
import { describeValue, KIND } from "./provenance";

/** Pure helpers for editing one override layer (the hub merges; the UI only edits a layer). */

export const emptyLayer = (): OverrideLayer => ({ items: [], extra_instructions: null, env: [], disallowed_tools: [] });
export const sameScope = (a: LayerScope, b: LayerScope) => JSON.stringify(a) === JSON.stringify(b);
export const layerOf = (all: StoredLayer[], scope: LayerScope): OverrideLayer => all.find((l) => sameScope(l.scope, scope))?.layer ?? emptyLayer();

const same = (o: ItemOverride, kind: ConfigItemKind, name: string) => o.kind === kind && o.name === name;

export function withOverride(layer: OverrideLayer, o: ItemOverride): OverrideLayer {
  return { ...layer, items: [...layer.items.filter((x) => !same(x, o.kind, o.name)), o] };
}
export function withoutOverride(layer: OverrideLayer, kind: ConfigItemKind, name: string): OverrideLayer {
  return { ...layer, items: layer.items.filter((x) => !same(x, kind, name)) };
}

/**
 * The switch in the effective table, edited in `layer`: an active item gets a `disable` here; an item
 * this layer switched off loses that override; an item a broader layer switched off is `reset` here.
 */
export function toggled(layer: OverrideLayer, it: EffectiveItem, scope: LayerScope): OverrideLayer {
  const own = layer.items.find((x) => same(x, it.kind, it.name));
  if (it.active) return withOverride(layer, { kind: it.kind, name: it.name, action: "disable" });
  if (own?.action === "disable") return withoutOverride(layer, it.kind, it.name);
  const byOther = it.changed_by?.from === "layer" && !sameScope(it.changed_by.scope, scope);
  return byOther ? withOverride(layer, { kind: it.kind, name: it.name, action: "reset" }) : layer;
}

/**
 * The chip a row offers for the selected layer: `remove` for an item this layer added (it goes
 * away), `reset` for an inherited item this layer changed or switched off (back to what the
 * broader layers say). Nothing otherwise.
 */
export function rowAction(it: EffectiveItem, scope: LayerScope | null): "remove" | "reset" | null {
  if (!scope || it.changed_by?.from !== "layer" || !sameScope(it.changed_by.scope, scope)) return null;
  if (it.state === "added" && it.origin.from === "layer" && sameScope(it.origin.scope, scope)) return "remove";
  return it.state === "changed" || it.state === "disabled" ? "reset" : null;
}

export interface LayerLine {
  op: "+" | "~" | "−" | "↺" | "·";
  cls: string;
  text: string;
}

export function summarize(layer: OverrideLayer, base: ConfigItem[]): LayerLine[] {
  const out: LayerLine[] = layer.items.map((o) => {
    const k = KIND[o.kind].one;
    if (o.action === "disable") return { op: "−", cls: "del", text: `${o.name} turned off` };
    if (o.action === "reset") return { op: "↺", cls: "faint", text: `${o.name} reset` };
    const inherited = base.some((b) => b.kind === o.kind && b.name === o.name);
    return inherited ? { op: "~", cls: "c-working", text: `${o.name} → ${describeValue(o.value)}` } : { op: "+", cls: "add", text: `${o.name} ${k}` };
  });
  if (layer.extra_instructions) out.push({ op: "+", cls: "add", text: "extra instructions" });
  for (const [k] of layer.env) out.push({ op: "~", cls: "c-working", text: `env ${k}` });
  if (layer.disallowed_tools.length) out.push({ op: "−", cls: "del", text: `${layer.disallowed_tools.length} tool${layer.disallowed_tools.length > 1 ? "s" : ""} not allowed` });
  return out;
}

export function counts(items: ConfigItem[]): string {
  const by = new Map<ConfigItemKind, number>();
  for (const i of items) by.set(i.kind, (by.get(i.kind) ?? 0) + 1);
  return [...by.entries()].map(([k, n]) => `${n} ${(n === 1 ? KIND[k].one : KIND[k].label.toLowerCase()).replace("mcp", "MCP")}`).join(" · ");
}

export function sources(items: ConfigItem[]): { path: string; what: string }[] {
  const m = new Map<string, Set<string>>();
  for (const i of items) {
    const p = i.source.path ?? i.source.scope;
    if (!m.has(p)) m.set(p, new Set());
    m.get(p)!.add(KIND[i.kind].label.toLowerCase());
  }
  return [...m.entries()].map(([path, w]) => ({ path, what: [...w].join(", ") }));
}
