import { linkView, useCounts, useHosts } from "../state";
import { money } from "../lib/format";
import { ConnState, Icon } from "../ui";
import { useNavigate } from "../router";

export function StatusBar({ onHelp }: { onHelp: () => void }) {
  const hosts = useHosts();
  const counts = useCounts();
  const nav = useNavigate();
  return (
    <footer className="statusbar" aria-label="Status">
      {hosts.map((h) => {
        const l = linkView(h.link, h.authPrompt);
        return l.connected ? (
          <span key={h.id} className="sb-item">
            <ConnState host={h} short />
            {h.name}
          </span>
        ) : (
          <button type="button" key={h.id} className={`sb-item ${l.tone === "bad" ? "c-failed" : l.tone === "info" ? "c-working" : "c-needs"}`} data-link={l.key} onClick={() => nav(`/hosts?h=${encodeURIComponent(h.id)}`)}>
            <Icon name={l.tone === "bad" ? "warn" : "refresh"} size="xs" />
            {h.name} · {l.word.toLowerCase()}
          </button>
        );
      })}
      <span className="sp" />
      {counts.failed ? (
        <span className="sb-item c-failed">
          <Icon name="failed" size="xs" />
          {counts.failed} failed
        </span>
      ) : null}
      <span className="sb-item tnum">
        Today {money(counts.todayCost)} · {counts.todaySessions} sessions
      </span>
      <button type="button" className="sb-item" onClick={onHelp}>
        <kbd>?</kbd> Shortcuts
      </button>
    </footer>
  );
}
