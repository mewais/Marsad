import type { HostEntry } from "../../bridge/wire";
import { linkView, type HostLink } from "../../state";

/** A line under the link state: what is going on, from the link's own fields. */
export function linkSub(link: HostLink, now = Date.now()): string {
  if (link.link === "local") return link.hub && link.hub.state !== "connected" ? linkView(link, null, now).detail : "This machine";
  if (link.stale) return linkView(link, null, now).detail;
  const l = link.state;
  switch (l.state) {
    case "connected":
      return `helper ${l.hello.build}`;
    case "reconnecting":
      return `attempt ${l.attempt} · ${l.last_error}`;
    case "provisioning":
      return l.reason.kind === "build_mismatch" ? `${l.reason.found} → ${l.reason.expected}` : l.reason.kind === "command_not_found" ? "first connection" : "protocol update";
    case "auth_needed":
      return l.held && l.cause ? `dropped: ${l.cause}` : l.detail;
    case "failed":
      return l.reason;
    default:
      return linkView(link, null, now).detail;
  }
}

export function address(e: HostEntry): string {
  const k = e.kind;
  return k.kind === "local" ? "this machine" : k.kind === "ssh" ? `${k.target}${k.port ? `:${k.port}` : ""}` : `WSL ${k.distro}`;
}

/** A feed refusal's text as one short clause: no line breaks, no trailing full stop, no dump. */
export function feedReason(text: string): string {
  const one = text.replace(/\s+/g, " ").trim().replace(/[.\s]+$/, "");
  return one.length > 160 ? `${one.slice(0, 157)}…` : one;
}
