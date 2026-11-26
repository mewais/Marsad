import { linkView, type HostView } from "../state/hosts";
import type { FleetLink } from "../bridge/wire";
import { Icon, type IconName } from "./Icon";
import { Elapsed, useNow } from "./Elapsed";

/**
 * A connect in progress, in words: "Connecting · <stage> · <elapsed>" (the elapsed clock runs from
 * the hub's `since_ms`), or a bare "Connecting" when the hub named no stage.
 */
export function ConnectingWords({ stage, since }: { stage?: string; since?: number }) {
  if (!stage || since === undefined) return <>Connecting</>;
  return <>Connecting · {stage} · <Elapsed t={{ started_at_ms: since, running: true }} /></>;
}

export function hostIcon(h: Pick<HostView, "local" | "os" | "kind">): IconName {
  if (h.os === "windows" || h.kind.kind === "wsl") return "monitor";
  return h.local ? "laptop" : "server";
}

/** The link state in words, never just a dot. */
export function ConnState({ host, short }: { host: Pick<HostView, "link" | "authPrompt">; short?: boolean }) {
  const ticking = host.link.link === "remote" && host.link.state.state === "reconnecting" && !short;
  const now = useNow(ticking);
  const v = linkView(host.link as FleetLink, host.authPrompt, now);
  const st = host.link.link === "remote" ? host.link.state : undefined;
  const words =
    short ? v.word
    : st?.state === "connecting" && v.key === "connecting" ? <ConnectingWords stage={st.stage} since={st.since_ms} />
    : v.key === "auth_needed" || v.key === "failed" ? v.word
    : v.detail.split(" · ")[0];
  return (
    <span className={`conn ${v.tone}`} data-link={v.key} data-held={v.held ? "" : undefined}>
      <span className="pip" aria-hidden="true" />
      {words}
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
