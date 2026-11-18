import { useState } from "react";
import type { WslDistros } from "../../bridge/wire";
import { Icon, TextField } from "../../ui";

/** Detected WSL distributions as a choice (WSL's default first and preselected by the caller), with typing a name as the fallback. */
export function WslDistroPicker({ list, value, onChange }: { list: WslDistros | null; value: string; onChange: (distro: string) => void }) {
  const names = list?.distros.map((d) => d.name) ?? [];
  const [manual, setManual] = useState(false);
  const typing = manual || (list !== null && names.length === 0) || (value !== "" && !names.includes(value));
  if (!list) return <div className="faint small" role="status">Looking for WSL distributions…</div>;
  return (
    <div className="field">
      <span className="lbl" id="wsl-distro-lbl">WSL distribution</span>
      {names.length ? (
        <div className="sugg" role="radiogroup" aria-labelledby="wsl-distro-lbl">
          {list.distros.map((d) => (
            <button key={d.name} type="button" role="radio" aria-checked={!typing && value === d.name} className={`chip${!typing && value === d.name ? " on" : ""}`} onClick={() => { setManual(false); onChange(d.name); }}>
              <Icon name="monitor" size="xs" />{d.name}
              {d.default ? <span className="faint"> · default</span> : null}
              {d.running ? <span className="faint"> · running</span> : null}
            </button>
          ))}
          <button type="button" role="radio" aria-checked={typing} className={`chip${typing ? " on" : ""}`} onClick={() => { setManual(true); onChange(""); }}>Other…</button>
        </div>
      ) : null}
      {list.problem ? <div className="faint small">{list.problem}</div> : null}
      {typing ? <TextField label="Distribution name" hint="As `wsl -l` lists it" value={value} onChange={(e) => onChange(e.target.value)} placeholder="Ubuntu-24.04" spellCheck={false} /> : null}
    </div>
  );
}

/** The distribution to preselect: WSL's default, else the first one. */
export function defaultDistro(list: WslDistros | null): string {
  return list?.distros.find((d) => d.default)?.name ?? list?.distros[0]?.name ?? "";
}
