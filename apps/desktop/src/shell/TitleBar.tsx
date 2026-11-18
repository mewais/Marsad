import { useCounts } from "../state";
import { useNavigate } from "../router";
import { IconButton, SplitButton, Icon, Kbd } from "../ui";
import { openNewSession, openNewWorkstream } from "./dialogs";

export function TitleBar({ onPalette }: { onPalette: () => void }) {
  const counts = useCounts();
  const nav = useNavigate();
  return (
    <header className="titlebar">
      <div className="brand">
        <Icon name="tower" size="lg" className="c-accent" />
        Marsad <span className="ar" lang="ar">مرصد</span>
      </div>
      <button type="button" className="cmdk" onClick={onPalette} aria-label="Search sessions, hosts, commands" aria-keyshortcuts="Control+K Meta+K">
        <Icon name="search" size="sm" />
        <span>Search sessions, hosts, commands</span>
        <Kbd>Ctrl K</Kbd>
      </button>
      <div className="tb-right">
        <IconButton icon="bell" label="Notifications" badge={counts.needs || undefined} onClick={() => nav(counts.needs ? "/inbox" : "/settings?s=notifications")} />
        <SplitButton
          label="New session"
          icon="plus"
          kbd="N"
          onClick={() => openNewSession()}
          menuLabel="More ways to start"
          items={[
            { id: "s", label: "New session", description: "One agent on one host", icon: "plus", kbd: "N", onSelect: () => openNewSession() },
            { id: "w", label: "New workstream", description: "Several agents that hand work to each other", icon: "graph", kbd: "⇧N", onSelect: () => openNewWorkstream() },
          ]}
        />
      </div>
    </header>
  );
}
