import { useEffect, useState } from "react";
import { linkView, type HostView } from "../state/hosts";
import type { FleetLink } from "../bridge/wire";
import { Icon, type IconName } from "./Icon";

export function hostIcon(h: Pick<HostView, "local" | "os" | "kind">): IconName {
  if (h.os === "windows" || h.kind.kind === "wsl") return "monitor";
  return h.local ? "laptop" : "server";
}

/** Re-render every `ms` while `on` (countdowns). */
export function useNow(on: boolean, ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const h = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(h);
  }, [on, ms]);
  return now;
}

/** The link state in words, never just a dot. */
export function ConnState({ host, short }: { host: Pick<HostView, "link" | "authPrompt">; short?: boolean }) {
  const ticking = host.link.link === "remote" && host.link.state.state === "reconnecting" && !short;
  const now = useNow(ticking);
  const v = linkView(host.link as FleetLink, host.authPrompt, now);
  return (
    <span className={`conn ${v.tone}`} data-link={v.key}>
      <span className="pip" aria-hidden="true" />
      {short || v.key === "auth_needed" || v.key === "failed" ? v.word : v.detail.split(" · ")[0]}
    </span>
  );
}

export interface HostChipProps {
  host: HostView;
  /** Show the link state next to the name. */
  showLink?: boolean | "unless-connected";
  shortLink?: boolean;
}

export function HostChip({ host, showLink = false, shortLink }: HostChipProps) {
  const connected = linkView(host.link, host.authPrompt).connected;
  const link = showLink === true || (showLink === "unless-connected" && !connected);
  return (
    <span className="host">
      <Icon name={hostIcon(host)} size="sm" />
      <span className="hn" title={host.name}>{host.name}</span>
      {link ? <ConnState host={host} short={shortLink} /> : null}
    </span>
  );
}
