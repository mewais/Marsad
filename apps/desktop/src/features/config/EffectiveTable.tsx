import type { ConfigItemKind, EffectiveConfig, EffectiveItem, LayerScope } from "../../bridge/wire";
import { Icon } from "../../ui";
import { rowAction } from "./layers";
import { describeItem, KIND, KIND_ORDER, provenance, sourceOf } from "./provenance";

/**
 * The effective config grouped by kind: provenance badges, sources, greyed unsupported items with
 * reasons, switches. With `scope` (the layer being edited) and `onUndo`, an item that layer added
 * gets a "remove" chip (instead of a switch) and one it changed or switched off a "reset" chip.
 */
export function EffectiveTable({ eff, driverName, filter, onToggle, busy, scope = null, onUndo }: { eff: EffectiveConfig; driverName: string; filter: ConfigItemKind | "all"; onToggle: (it: EffectiveItem) => void; busy: boolean; scope?: LayerScope | null; onUndo?: (it: EffectiveItem) => void }) {
  const groups = KIND_ORDER.filter((k) => filter === "all" || k === filter).map((k) => ({ k, items: eff.items.filter((i) => i.kind === k) })).filter((g) => g.items.length);
  return (
    <table className="etab">
      <thead><tr><th>Item</th><th>Where it comes from</th><th>Source</th><th className="onc">On</th></tr></thead>
      <tbody>
        {groups.map((g) => [
          <tr key={g.k} className="cat"><td colSpan={4}><Icon name={KIND[g.k].icon} size="sm" /> {KIND[g.k].label}</td></tr>,
          ...g.items.map((it) => { const act = rowAction(it, scope); return (
            <tr key={`${it.kind}:${it.name}`} className={`${it.active ? "" : "off"}${it.unsupported ? " uns" : ""}${it.state === "automatic" ? " wasl" : ""}`} data-item={`${it.kind}:${it.name}`}>
              <td>
                <div className="nm">{it.name}{it.locked ? <Icon name="lock" size="xs" className="faint" title="Marsad adds this; it cannot be overridden" /> : null}</div>
                <div className="ds">{describeItem(it)}</div>
                {it.unsupported ? <div className="reason keep"><Icon name="minus" size="xs" /> {it.unsupported}</div> : null}
              </td>
              <td className="keep"><div className="provs">{provenance(it, driverName).map((b) => <span key={b.text} className={`prov ${b.cls}`}>{b.cls === "wasl" ? <Icon name="handoff" size="xs" /> : null}{b.text}</span>)}
                {act && onUndo ? <button type="button" className={`prov chipbtn ${act}`} disabled={busy} onClick={() => onUndo(it)} aria-label={act === "remove" ? `Remove ${it.name} from this layer` : `Reset ${it.name} in this layer`}><Icon name={act === "remove" ? "x" : "refresh"} size="xs" />{act}</button> : null}</div></td>
              <td className="src">{sourceOf(it)}</td>
              <td className="keep onc">
                {it.locked ? <span className="lockcell"><Icon name="lock" size="xs" />auto</span>
                  : act === "remove" ? null
                  : <button type="button" role="switch" className="tg" aria-checked={it.active} disabled={busy} aria-label={`${it.name} on for launches in the selected layer`} onClick={() => onToggle(it)} />}
              </td>
            </tr>
          ); }),
        ])}
        {groups.length === 0 ? <tr><td colSpan={4} className="faint">Nothing of this kind.</td></tr> : null}
      </tbody>
    </table>
  );
}
