import { useId } from "react";
import { Icon } from "./Icon";

export interface CheckboxProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "type" | "onChange"> {
  label: React.ReactNode;
  hint?: string;
  onChange: (checked: boolean) => void;
}

export function Checkbox({ label, hint, onChange, className = "", ...rest }: CheckboxProps) {
  return (
    <label className={`check-row ${className}`}>
      <input type="checkbox" onChange={(e) => onChange(e.target.checked)} {...rest} />
      <span className="cb" aria-hidden="true">
        <Icon name="check" size="xs" />
      </span>
      <span>
        {label}
        {hint ? <span className="hint">{hint}</span> : null}
      </span>
    </label>
  );
}

export interface RadioOption<T extends string> {
  value: T;
  label: React.ReactNode;
  hint?: string;
  disabled?: boolean;
}

export interface RadioGroupProps<T extends string> {
  label: string;
  value: T;
  onChange: (v: T) => void;
  options: RadioOption<T>[];
}

/** Native radios (arrow-key navigation for free) with the mockup's look. */
export function RadioGroup<T extends string>({ label, value, onChange, options }: RadioGroupProps<T>) {
  const name = useId();
  return (
    <fieldset className="group">
      <legend className="sr-only">{label}</legend>
      {options.map((o) => (
        <label key={o.value} className="check-row">
          <input type="radio" name={name} value={o.value} checked={o.value === value} disabled={o.disabled} onChange={() => onChange(o.value)} />
          <span className="rad-dot" aria-hidden="true" />
          <span>
            {o.label}
            {o.hint ? <span className="hint">{o.hint}</span> : null}
          </span>
        </label>
      ))}
    </fieldset>
  );
}
