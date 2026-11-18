import { useStore } from "../lib/store";
import { prettyKey, shortcutStore } from "../shortcuts/registry";
import { allShortcuts } from "../shortcuts/catalog";
import { Dialog, Kbd } from "../ui";

export function ShortcutHelp({ open, onClose }: { open: boolean; onClose: () => void }) {
  const list = allShortcuts(useStore(shortcutStore, (s) => s));
  const groups = new Map<string, typeof list>();
  for (const s of list) groups.set(s.group, [...(groups.get(s.group) ?? []), s]);
  return (
    <Dialog open={open} onClose={onClose} title="Keyboard shortcuts" className="help">
      <div className="help-grid">
        {[...groups.entries()].map(([g, items]) => (
          <div key={g} className="contents">
            <div className="eyebrow help-h">{g}</div>
            {items.map((s) => (
              <div key={s.id} className="help-row">
                <span>{s.title}</span>
                <span className="keys">
                  {s.keys.map((k, i) => (
                    <Kbd key={i}>{prettyKey(k)}</Kbd>
                  ))}
                </span>
              </div>
            ))}
          </div>
        ))}
      </div>
    </Dialog>
  );
}
