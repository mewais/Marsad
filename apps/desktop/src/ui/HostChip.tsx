import { linkView, type HostView } from "../state/hosts";
import type { FleetLink } from "../bridge/wire";
import { Icon, type IconName } from "./Icon";
import { Elapsed, useNow } from "./Elapsed";

/**
 * A connect in progress: header row ("Connecting" + right-aligned tabular elapsed time) with the
 * hub's stage on its own single-line ellipsized row and the full wording in a tooltip. Without a
 * stage (or `since`), renders the bare header and optional fallback line.
 */
export function ConnectingWords({
  stage,
  since,
  fallback,
  icon,
}: {
  stage?: string;
  since?: number;
  fallback?: string;
  icon?: React.ReactNode;
}) {
  const hasStage = !!stage && since !== undefined;
  return (
    <div className="conn-prog" data-testid="connecting-progress">
      <div className="conn-prog-head">
        {icon}
        <span className="conn-prog-word">Connecting</span>
        {hasStage ? (
          <span className="conn-prog-elapsed tnum" data-testid="connecting-elapsed">
            <Elapsed t={{ started_at_ms: since, running: true }} />
          </span>
        ) : null}
      </div>
      {hasStage ? (
        <div className="conn-prog-stage small" title={stage} data-testid="connecting-stage">
          {stage}
        </div>
      ) : fallback ? (
        <div className="conn-prog-stage small">{fallback}</div>
      ) : null}
    </div>
  );
}

export function hostIcon(h: Pick<HostView, "local" | "os" | "kind">): IconName {
  if (h.os === "windows" || h.kind.kind === "wsl") return "monitor";
  return h.local ? "laptop" : "server";
}

/**
 * The link state in words, never just a dot. A connect in progress keeps the chip compact
 * ("Connecting" + tabular elapsed time, full stage in `title`) so long stage wording never
 * squeezes adjacent content.
 */
export function ConnState({ host, short }: { host: Pick<HostView, "link" | "authPrompt">; short?: boolean }) {
  const ticking = host.link.link === "remote" && host.link.state.state === "reconnecting" && !short;
  const now = useNow(ticking);
  const v = linkView(host.link as FleetLink, host.authPrompt, now);
  const st = host.link.link === "remote" ? host.link.state : undefined;
  const connectingWithStage = !short && st?.state === "connecting" && v.key === "connecting" && !!st.stage;
  const label =
    short || v.key === "auth_needed" || v.key === "failed" || v.key === "connecting" ? v.word
    : v.detail.split(" · ")[0];
  const tip = st?.state === "connecting" && st.stage ? st.stage : undefined;
  return (
    <span className={`conn ${v.tone}`} data-link={v.key} data-held={v.held ? "" : undefined} title={tip}>
      <span className="pip" aria-hidden="true" />
      <span className="conn-word">{label}</span>
      {connectingWithStage ? (
        <span className="conn-elapsed tnum">
          <Elapsed t={{ started_at_ms: st.since_ms, running: true }} />
        </span>
      ) : null}
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
