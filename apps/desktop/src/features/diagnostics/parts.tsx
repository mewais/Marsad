import { Button, CopyButton, Icon } from "../../ui";

/** The Diagnostics panels' CSS lives next to them. */
import "./diagnostics.css";

/** `<h4>` of a panel section with an optional note and actions at its right. */
export function SectionHead({ title, note, children }: { title: string; note?: string; children?: React.ReactNode }) {
  return (
    <div className="dg-sh">
      <h4 className="eyebrow">{title}</h4>
      {note ? <span className="faint small">{note}</span> : null}
      <span className="sp" />
      {children}
    </div>
  );
}

/** A key / value list whose values can be copied (`value: null`: the daemon has not reported it). */
export function Facts({ facts }: { facts: { k: string; v: string | null; copy?: boolean }[] }) {
  return (
    <dl className="dg-facts">
      {facts.map(({ k, v, copy }) => (
        <div key={k} className="dg-fact">
          <dt>{k}</dt>
          <dd>
            {v === null ? <span className="faint">not reported</span> : <span className="mono dg-val">{v}</span>}
            {v !== null && copy !== false ? <CopyButton text={v} label={`Copy ${k.toLowerCase()}`} /> : null}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** Pending, in words: something has been asked for and has not answered. */
export function Reading({ what }: { what: string }) {
  return (
    <div className="faint small dg-reading" role="status">
      <Icon name="working" size="xs" spin /> {what}
    </div>
  );
}

/** Why what is shown is not live, with the way to ask again. */
export function Problem({ title, detail, onRetry }: { title: string; detail: string; onRetry?: () => void }) {
  return (
    <div className="dg-problem" role="alert">
      <Icon name="warn" size="sm" />
      <div className="dg-pt">
        <b>{title}</b>
        <span className="small"> {detail}</span>
      </div>
      {onRetry ? <Button size="sm" icon="refresh" onClick={onRetry}>Try again</Button> : null}
    </div>
  );
}

/** The panel cannot read its data now (no link to the daemon, or the host is not connected): why, how to connect again, and that it reads again by itself once it can. */
export function Unreachable({ reason, retry }: { reason: string; retry?: { label: string; run: () => void; pending: boolean } | null }) {
  return (
    <div className="dg-problem warn" role="status" data-testid="diag-unreachable">
      <Icon name="clock" size="sm" />
      <div className="dg-pt">
        <b>Not read.</b>
        <span className="small"> {reason} This reads again by itself once it is reachable.</span>
      </div>
      {retry ? <Button size="sm" icon="refresh" pending={retry.pending} onClick={retry.run}>{retry.label}</Button> : null}
    </div>
  );
}
