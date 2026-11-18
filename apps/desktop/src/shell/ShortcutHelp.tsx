import { useStore } from "../lib/store";
import { prettyKey, shortcutStore } from "../shortcuts/registry";
import { Dialog, Kbd } from "../ui";

export function ShortcutHelp({ open, onClose }: { open: boolean; onClose: () => void }) {
  const list = useStore(shortcutStore, (s) => s);
  const groups = new Map<string, typeof list>();
  for (const s of list) groups.set(s.group, [...(groups.get(s.group) ?? []), s]);
  return (
    <Dialog open={open} onClose={onClose} title="Keyboard shortcuts" className="help">
      <div className="help-grid">
        {[...groups.entries()].map(([g, items]) => (
          <div key={g} style={{ display: "contents" }}>
            <div className="eyebrow help-h">{g}</div>
            {items.map((s) => (
              <div key={s.id} className="help-row">
                <span>{s.title}</span>
                <span style={{ display: "inline-flex", gap: 4 }}>
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
