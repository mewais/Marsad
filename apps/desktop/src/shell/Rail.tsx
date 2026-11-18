import { useCounts } from "../state";
import { useNavigate, useRoute } from "../router";
import { Icon, type IconName } from "../ui";

const ITEMS: { to: string; label: string; icon: IconName; match: string[] }[] = [
  { to: "/inbox", label: "Inbox", icon: "inbox", match: ["inbox", "session"] },
  { to: "/workstreams", label: "Workstreams", icon: "graph", match: ["workstreams", "workstream"] },
  { to: "/hosts", label: "Hosts", icon: "server", match: ["hosts"] },
  { to: "/config", label: "Config", icon: "layers", match: ["config"] },
];

export function Rail() {
  const route = useRoute();
  const nav = useNavigate();
  const counts = useCounts();
  const item = (to: string, label: string, icon: IconName, current: boolean, badge?: number) => (
    <a
      key={label}
      href={`#${to}`}
      aria-current={current ? "page" : undefined}
      onClick={(e) => {
        e.preventDefault();
        nav(to);
      }}
    >
      <Icon name={icon} size="lg" />
      {label}
      {badge ? <span className="badge" aria-label={`${badge} need you`}>{badge}</span> : null}
    </a>
  );
  return (
    <nav className="rail" aria-label="Main">
      {ITEMS.map((i) => item(i.to, i.label, i.icon, i.match.includes(route.name), i.label === "Inbox" ? counts.needs : undefined))}
      <span className="sp" />
      {item("/settings", "Settings", "gear", route.name === "settings")}
    </nav>
  );
}
