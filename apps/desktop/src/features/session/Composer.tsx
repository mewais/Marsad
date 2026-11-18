import { useMemo, useRef, useState } from "react";
import type { SessionSnapshot } from "../../bridge/wire";
import { useActions, usePending } from "../../state";
import { Button, Icon, IconButton, Kbd, Select } from "../../ui";

/** Message box: slash menu, mode and model pickers all come from the session snapshot. */
export function Composer({ sessionKey, snap }: { sessionKey: string; snap: SessionSnapshot }) {
  const actions = useActions();
  const [text, setText] = useState("");
  const [active, setActive] = useState(0);
  const ta = useRef<HTMLTextAreaElement>(null);
  const sending = usePending(`cmd:${sessionKey}:prompt`) === "pending";
  const slash = text.startsWith("/") && !text.includes(" ") && !text.includes("\n");
  const items = useMemo(() => (slash ? snap.slash_commands.filter((c) => c.name.toLowerCase().startsWith(text.slice(1).toLowerCase())) : []), [slash, text, snap.slash_commands]);
  const live = snap.phase.state !== "exited" && snap.phase.state !== "lost";

  const submit = async () => {
    const t = text.trim();
    if (!t || sending) return;
    const m = /^\/(\S+)\s*(.*)$/s.exec(t);
    const cmd = m && snap.slash_commands.some((c) => c.name === m[1]) ? m : null;
    setText("");
    const ok = cmd ? await actions.command(sessionKey, { command: "run_slash", name: cmd[1], args: cmd[2] }, { failTitle: "Could not run the command" }) : await actions.sendPrompt(sessionKey, t);
    if (!ok) setText(t);
  };
  const pick = (name: string) => {
    setText(`/${name} `);
    ta.current?.focus();
  };

  return (
    <div className="composer-wrap">
      <div className="composer">
        {slash && snap.slash_commands.length ? (
          <div className="slash" role="listbox" aria-label="Slash commands">
            {items.length === 0 ? <div className="faint" style={{ padding: 8 }}>No command starts with “{text}”</div> : null}
            {items.map((c, i) => (
              <div key={c.name} role="option" aria-selected={i === active} className={`it ${i === active ? "on" : ""}`} onMouseDown={(e) => { e.preventDefault(); pick(c.name); }}>
                <b>/{c.name}</b>
                <span>{c.description}</span>
                <span className="src">{c.argument_hint}</span>
              </div>
            ))}
          </div>
        ) : null}
        <textarea
          ref={ta}
          aria-label="Message"
          placeholder={live ? "Message the agent" : "This session has ended; sending continues it"}
          value={text}
          onChange={(e) => { setText(e.target.value); setActive(0); }}
          onKeyDown={(e) => {
            if (slash && items.length) {
              if (e.key === "ArrowDown") { e.preventDefault(); return setActive((a) => Math.min(items.length - 1, a + 1)); }
              if (e.key === "ArrowUp") { e.preventDefault(); return setActive((a) => Math.max(0, a - 1)); }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey && text.slice(1) !== items[active]?.name)) { e.preventDefault(); return pick(items[active].name); }
            }
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void submit(); }
            if (e.key === "Escape") (e.target as HTMLElement).blur();
          }}
        />
        <div className="comp-bar">
          <IconButton icon="slash" label="Slash commands" onClick={() => { setText("/"); ta.current?.focus(); }} disabled={!snap.capabilities.slash_commands} />
          {snap.available_modes.length ? (
            <Select compact up label="Mode" leading={<Icon name="shield" size="xs" />} value={snap.mode ?? undefined} options={snap.available_modes.map((m) => ({ value: m.id, label: m.label, description: m.description }))}
              onChange={(v) => void actions.command(sessionKey, { command: "set_mode", mode: v }, { failTitle: "Could not change the mode" })} />
          ) : null}
          {snap.available_models.length ? (
            <Select compact up label="Model" leading={<Icon name="sparkle" size="xs" />} value={snap.model ?? undefined} options={snap.available_models.map((m) => ({ value: m.id, label: m.display_name || m.id, description: m.description }))}
              onChange={(v) => void actions.command(sessionKey, { command: "set_model", model: v }, { failTitle: "Could not change the model" })} />
          ) : null}
          <span className="sp" />
          <span className="faint" style={{ fontSize: 11.5 }}><Kbd>Enter</Kbd> send · <Kbd>⇧ Enter</Kbd> new line</span>
          <Button variant="primary" size="sm" icon="send" pending={sending} disabled={!text.trim()} onClick={() => void submit()}>Send</Button>
        </div>
      </div>
    </div>
  );
}
