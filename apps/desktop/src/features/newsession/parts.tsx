import type { DriverInventory } from "../../bridge/wire";
import { agoLong, basename } from "../../lib/format";
import { linkView, osWord, useAppState, type HostView } from "../../state";
import { ConnState, DriverBadge, Icon, hostIcon, makeDriver } from "../../ui";
import type { ResumeRow } from "./model";

/** Agent picker: every driver the host's inventory knows, with version and the host's agent command when it has one; missing ones say why. */
export function DriverPick({ drivers, value, onChange }: { drivers: DriverInventory[]; value: string; onChange: (id: string) => void }) {
  const known = useAppState((s) => s.drivers);
  return (
    <div className="drvpick" role="radiogroup" aria-label="Agent">
      {drivers.map((d) => (
        <button key={d.driver} type="button" role="radio" aria-checked={d.driver === value} disabled={!d.installed} title={d.installed ? d.binary_path ?? undefined : d.error ?? "Not installed"}
          className={`dp${d.driver === value ? " on" : ""}`} onClick={() => onChange(d.driver)}>
          <DriverBadge driver={known[d.driver] ?? makeDriver(d.driver, d.display_name)} />
          <span className="v">{d.installed ? `v${d.version ?? "?"}` : d.refreshing && !d.refreshed_at_ms ? "checking…" : "not installed"}</span>
          {d.command ? <span className={`cmd${d.warning ? " c-needs" : ""}`} title={d.warning ?? `This host runs ${d.command} for ${d.display_name}`}>{d.warning ? <Icon name="warn" size="xs" /> : null}<span>{d.command}</span></span> : null}
        </button>
      ))}
      {drivers.length === 0 ? <span className="faint">No agent CLIs reported by this host yet.</span> : null}
    </div>
  );
}

export function HostPick({ hosts, value, onChange }: { hosts: HostView[]; value: string; onChange: (id: string) => void }) {
  return (
    <div className="hostpick" role="radiogroup" aria-label="Host">
      {hosts.map((h) => {
        const up = linkView(h.link, h.authPrompt).connected;
        return (
          <button key={h.id} type="button" role="radio" aria-checked={h.id === value} disabled={!up} className={`hp${h.id === value ? " on" : ""}${up ? "" : " dis"}`} onClick={() => onChange(h.id)}>
            <Icon name={hostIcon(h)} size="sm" />
            <b>{h.name}</b>
            <span className="faint">{[osWord(h.os), h.local ? "local" : h.kind.kind === "ssh" ? "SSH" : "WSL"].filter(Boolean).join(" · ")}</span>
            <span className="sp" />
            <ConnState host={h} />
          </button>
        );
      })}
    </div>
  );
}

/** The CLI's own sessions for host + folder: newest first, including ones started in a terminal. */
export function ResumeList({ rows, value, onChange }: { rows: ResumeRow[]; value: string; onChange: (id: string) => void }) {
  return (
    <div className="slist" role="listbox" aria-label="Sessions to continue">
      <div className="sh" aria-hidden="true"><span /><span>Title · first prompt</span><span>Started in</span><span className="r">Last</span></div>
      {rows.map((r) => (
        <div key={r.id} role="option" aria-selected={r.id === value} tabIndex={r.id === value || (!value && r === rows[0]) ? 0 : -1} className={`sr${r.id === value ? " on" : ""}`}
          onClick={() => onChange(r.id)}
          onKeyDown={(e) => {
            const i = rows.indexOf(r);
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const n = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
              onChange(n.id);
              (e.currentTarget.parentElement?.querySelector(`[data-id="${n.id}"]`) as HTMLElement | null)?.focus();
            } else if (e.key === " " || e.key === "Enter") {
              e.preventDefault();
              onChange(r.id);
            }
          }}
          data-id={r.id}>
          <span>{r.id === value ? <Icon name="check" size="xs" className="c-accent" /> : null}</span>
          <span className="tt" title={`Session ${r.id}`}><b className={r.title ? undefined : "faint"}>{r.title || "(no title)"}</b><span className="mono">{r.project ? basename(r.project) : ""}</span></span>
          <span>{r.startedIn === "terminal" ? <span className="tag trm"><Icon name="term" size="xs" />Terminal</span> : <span className="tag"><Icon name="tower" size="xs" />Marsad</span>}</span>
          <span className="r">{r.modified_ms ? agoLong(r.modified_ms) : "unknown"}{r.message_count != null ? <span className="faint mono msgs">{r.message_count} msg</span> : null}</span>
        </div>
      ))}
      {rows.length === 0 ? <div className="faint sempty">No sessions match.</div> : null}
    </div>
  );
}
