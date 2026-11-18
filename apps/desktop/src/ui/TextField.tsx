import { forwardRef, useId } from "react";
import { Icon, type IconName } from "./Icon";
import { Kbd } from "./Kbd";

export interface TextFieldProps extends Omit<React.InputHTMLAttributes<HTMLInputElement>, "size"> {
  label?: string;
  hint?: string;
  error?: string;
  icon?: IconName;
  kbd?: string;
  /** Accessible name when no visible label. */
  "aria-label"?: string;
  width?: number | string;
}

export const TextField = forwardRef<HTMLInputElement, TextFieldProps>(function TextField(
  { label, hint, error, icon, kbd, width, className = "", id, ...rest },
  ref,
) {
  const auto = useId();
  const fid = id ?? auto;
  const describedBy = [hint ? `${fid}-h` : "", error ? `${fid}-e` : ""].filter(Boolean).join(" ") || undefined;
  const box = (
    <div className={`input ${className}`} style={width != null ? { width } : undefined}>
      {icon ? <Icon name={icon} size="sm" /> : null}
      <input ref={ref} id={fid} aria-invalid={error ? true : undefined} aria-describedby={describedBy} {...rest} />
      {kbd ? <Kbd>{kbd}</Kbd> : null}
    </div>
  );
  if (!label && !hint && !error) return box;
  return (
    <div className="field">
      {label ? <label htmlFor={fid}>{label}</label> : null}
      {box}
      {hint ? (
        <span id={`${fid}-h`} className="hint">
          {hint}
        </span>
      ) : null}
      {error ? (
        <span id={`${fid}-e`} className="err" role="alert">
          {error}
        </span>
      ) : null}
    </div>
  );
});

export const SearchField = forwardRef<HTMLInputElement, Omit<TextFieldProps, "icon" | "type">>(function SearchField(props, ref) {
  return <TextField ref={ref} icon="search" type="search" autoComplete="off" spellCheck={false} {...props} />;
});
