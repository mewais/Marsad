import { useEffect, useMemo, useRef, useState } from "react";
import { actionStore, searchActions } from "../commands/registry";
import { useStore } from "../lib/store";
import { sessionPath, useNavigate } from "../router";
import { selectView, useApp, type SessionView } from "../state";
import { Dialog, Icon, Kbd } from "../ui";

interface Item {
  id: string;
  section: string;
  title: string;
  hint?: string;
  icon: "search" | "inbox" | "term";
  run: () => void;
}

export function CommandPalette({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { store } = useApp();
  const nav = useNavigate();
  const actions = useStore(actionStore, (s) => s);
  const sessions = useStore(store, (s) => s.sessions);
  const hosts = useStore(store, (s) => s.links);
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQ("");
      setActive(0);
    }
  }, [open]);

  const items = useMemo<Item[]>(() => {
    const out: Item[] = searchActions(actions, q).map((a) => ({ id: a.id, section: a.section, title: a.title, hint: a.shortcut, icon: (a.icon as Item["icon"]) ?? "term", run: a.run }));
    const n = q.trim().toLowerCase();
    if (n) {
      const found: SessionView[] = [];
      for (const key of Object.keys(sessions)) {
        const v = selectView(store.get(), key)!;
        if (`${v.name} ${v.project} ${v.branch} ${hosts[v.host]?.name ?? ""}`.toLowerCase().includes(n)) found.push(v);
      }
      for (const v of found.slice(0, 8)) {
        out.push({ id: `s:${v.key}`, section: "Sessions", title: v.name, hint: `${hosts[v.host]?.name ?? v.host} · ${v.project}`, icon: "inbox", run: () => nav(sessionPath(v.host, v.id)) });
      }
    }
    return out;
  }, [actions, q, sessions, hosts, store, nav]);

  const choose = (it?: Item) => {
    if (!it) return;
    onClose();
    it.run();
  };
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(items.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(items[active]);
    }
  };

  let last = "";
  return (
    <Dialog open={open} onClose={onClose} title="Command palette" bare className="palette" initialFocus={input}>
      <div className="input" style={{ marginBottom: 6 }}>
        <Icon name="search" size="sm" />
        <input
          ref={input}
          role="combobox"
          aria-label="Search sessions, hosts, commands"
          aria-expanded="true"
          aria-controls="pal-list"
          aria-activedescendant={items[active] ? `pal-${items[active].id}` : undefined}
          placeholder="Search sessions, hosts, commands"
          value={q}
          onChange={(e) => {
            setQ(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKey}
        />
        <Kbd>Esc</Kbd>
      </div>
      <div id="pal-list" className="pal-list" role="listbox" aria-label="Results">
        {items.length === 0 ? <div className="faint" style={{ padding: 10 }}>No matches</div> : null}
        {items.map((it, i) => {
          const head = it.section !== last ? <div key={`h-${it.section}`} className="pal-sec" role="presentation">{it.section}</div> : null;
          last = it.section;
          return (
            <div key={it.id} style={{ display: "contents" }}>
              {head}
              <div id={`pal-${it.id}`} role="option" aria-selected={i === active} className="pal-it" onMouseEnter={() => setActive(i)} onClick={() => choose(it)}>
                <Icon name={it.icon} size="sm" />
                <span>{it.title}</span>
                {it.hint ? <span className="faint">{it.hint}</span> : <span />}
              </div>
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}
