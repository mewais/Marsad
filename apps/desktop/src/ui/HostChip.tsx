import type { HostLink, LinkState } from "../bridge/types";
import { Icon, type IconName } from "./Icon";

export function hostIcon(h: Pick<HostLink, "where" | "os">): IconName {
  if (h.os && /windows/i.test(h.os)) return "monitor";
  return h.where === "local" ? "laptop" : "server";
}

export function linkTone(link: LinkState): "ok" | "warn" | "bad" | "info" {
  return link === "connected" ? "ok" : link === "offline" ? "bad" : link === "connecting" ? "info" : "warn";
}

/** The link state in words, never just a dot. */
export function linkWord(h: Pick<HostLink, "link" | "retry_in_ms">, short = false): string {
  switch (h.link) {
    case "connected":
      return "Connected";
    case "connecting":
      return "Connecting";
    case "offline":
      return "Offline";
    case "reconnecting":
      if (short) return "Reconnecting";
      return h.retry_in_ms != null ? `Reconnecting in ${Math.max(1, Math.round(h.retry_in_ms / 1000))}s` : "Reconnecting";
  }
}

export function ConnState({ host, short }: { host: Pick<HostLink, "link" | "retry_in_ms">; short?: boolean }) {
  return (
    <span className={`conn ${linkTone(host.link)}`} data-link={host.link}>
      <span className="pip" aria-hidden="true" />
      {linkWord(host, short)}
    </span>
  );
}

export interface HostChipProps {
  host: HostLink;
  /** Show the link state next to the name (omit when it is connected and the context is crowded). */
  showLink?: boolean | "unless-connected";
  shortLink?: boolean;
}

export function HostChip({ host, showLink = false, shortLink }: HostChipProps) {
  const link = showLink === true || (showLink === "unless-connected" && host.link !== "connected");
  return (
    <span className="host">
      <Icon name={hostIcon(host)} size="sm" />
      <span>{host.name}</span>
      {link ? <ConnState host={host} short={shortLink} /> : null}
    </span>
  );
}
