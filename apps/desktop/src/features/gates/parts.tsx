import { useDriver } from "../../state";
import { DriverBadge, Icon, Button, Kbd } from "../../ui";
import type { GateChoice, LabelPart } from "./choices";

export function Label({ parts }: { parts: LabelPart[] }) {
  return (
    <>
      {parts.map((p, i) => (p.code ? <code key={i}>{p.text}</code> : <span key={i}>{p.text}</span>))}
    </>
  );
}

/** Whose wording the buttons use: the agent's own, Marsad's handoff wording, or neutral labels for an agent whose protocol sends none. */
export function SourceCaption({ source, driver }: { source: "agent" | "neutral" | "handoff" | "spawn"; driver?: string }) {
  const d = useDriver(driver ?? "");
  const name = d?.displayName ?? "The agent";
  if (source === "spawn")
    return (
      <span className="srccap wasl" title="An agent starting another agent: Marsad's own gate">
        <Icon name="sparkle" size="xs" />
        Marsad spawn choices
      </span>
    );
  if (source === "handoff")
    return (
      <span className="srccap wasl" title="Handoffs between agents: Marsad's own gate">
        <Icon name="handoff" size="xs" />
        Marsad handoff choices
      </span>
    );
  if (source === "neutral")
    return (
      <span className="srccap" title={`${name} offers exactly these choices but sends no wording for them, so the labels are neutral ones`}>
        {d ? <DriverBadge driver={d} compact /> : <Icon name="shield" size="xs" />}
        {name}'s choices · neutral labels
      </span>
    );
  return (
    <span className="srccap" title={`Shown exactly as ${name} offered them`}>
      {d ? <DriverBadge driver={d} compact /> : null}
      {name}'s own choices
    </span>
  );
}

export interface ChoiceButtonsProps {
  choices: GateChoice[];
  busy: boolean;
  /** Why the choices cannot be sent now (last-known data, no link): the buttons are off and say why. */
  blocked?: string | null;
  onChoose: (c: GateChoice) => void;
  keys?: boolean;
  size?: "sm" | "md";
}

export function ChoiceButtons({ choices, busy, blocked, onChoose, keys, size = "sm" }: ChoiceButtonsProps) {
  return (
    <div className="choices" role="group" aria-label="Answer choices" aria-busy={busy || undefined}>
      {choices.map((c, i) => (
        <Button key={c.id} size={size} variant={c.kind} disabled={busy} blocked={blocked} icon={i === 0 ? "check" : undefined} onClick={() => onChoose(c)} className={size === "md" ? "lg" : undefined} title={c.description || undefined} data-choice={c.id}>
          <Label parts={c.label} />
          {c.text === "required" && size === "sm" ? "…" : null}
          {keys ? <Kbd>{String(i + 1)}</Kbd> : null}
        </Button>
      ))}
      {busy ? (
        <span className="faint fs115" role="status">
          <Icon name="working" size="xs" spin /> Sending…
        </span>
      ) : null}
    </div>
  );
}

export function ReasonBox({ who, value, onChange, inputRef, onSubmit, label }: { who: string; value: string; onChange: (v: string) => void; inputRef: React.RefObject<HTMLInputElement>; onSubmit: () => void; label?: string }) {
  return (
    <div className="tell">
      <div className="input">
        <Icon name="edit" size="sm" />
        <input
          ref={inputRef}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              onSubmit();
            }
          }}
          placeholder={`Tell ${who} what to do differently (sent with “${label ?? "Deny"}”)`}
          aria-label={`Tell ${who} what to do differently`}
        />
      </div>
    </div>
  );
}
