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

/** "Claude Code's own choices" / "Marsad handoff choices": the human always knows whose wording it is. */
export function SourceCaption({ source, driver }: { source: "agent" | "marsad" | "handoff"; driver?: string }) {
  const d = useDriver(driver ?? "");
  if (source === "handoff")
    return (
      <span className="srccap wasl" title="Handoffs between agents always offer these three choices">
        <Icon name="handoff" size="xs" />
        Marsad handoff choices
      </span>
    );
  if (source === "marsad" || !d)
    return (
      <span className="srccap" title="This agent did not send its own wording, so Marsad shows its standard choices">
        <Icon name="shield" size="xs" />
        Marsad standard choices
      </span>
    );
  return (
    <span className="srccap" title={`Shown exactly as ${d.displayName} offered them`}>
      <DriverBadge driver={d} compact />
      {d.displayName}'s own choices
    </span>
  );
}

export interface ChoiceButtonsProps {
  choices: GateChoice[];
  busy: boolean;
  onChoose: (c: GateChoice) => void;
  keys?: boolean;
  size?: "sm" | "md";
}

export function ChoiceButtons({ choices, busy, onChoose, keys, size = "sm" }: ChoiceButtonsProps) {
  return (
    <div className="choices" role="group" aria-label="Answer choices" aria-busy={busy || undefined}>
      {choices.map((c, i) => (
        <Button key={c.id} size={size} variant={c.kind} disabled={busy} icon={i === 0 ? "check" : undefined} onClick={() => onChoose(c)} className={size === "md" ? "lg" : undefined}>
          <Label parts={c.label} />
          {c.asksReason && size === "sm" ? "…" : null}
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

export function ReasonBox({ who, value, onChange, inputRef, onSubmit }: { who: string; value: string; onChange: (v: string) => void; inputRef: React.RefObject<HTMLInputElement>; onSubmit: () => void }) {
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
          placeholder={`Tell ${who} what to do differently (sent with “No”)`}
          aria-label={`Tell ${who} what to do differently`}
        />
      </div>
    </div>
  );
}
