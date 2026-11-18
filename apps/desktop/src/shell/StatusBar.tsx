import { useCounts, useHostLinks } from "../state";
import { money } from "../lib/format";
import { ConnState, Icon, linkTone, linkWord } from "../ui";

export function StatusBar({ onHelp }: { onHelp: () => void }) {
  const hosts = useHostLinks();
  const counts = useCounts();
  return (
    <footer className="statusbar" aria-label="Status">
      {hosts.map((h) =>
        h.link === "connected" ? (
          <span key={h.id} className="sb-item">
            <ConnState host={{ link: "connected" }} short />
            {h.name}
          </span>
        ) : (
          <span key={h.id} className={`sb-item ${linkTone(h.link) === "bad" ? "c-failed" : "c-needs"}`} data-link={h.link}>
            <Icon name="refresh" size="xs" />
            {h.name} {linkWord(h).toLowerCase()}
          </span>
        ),
      )}
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
