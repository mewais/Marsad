import { useEffect, useState } from "react";
import { errorMessage } from "../../../bridge/api";
import type { EffectiveConfig, SessionSnapshot } from "../../../bridge/wire";
import { splitKey } from "../../../bridge/wire";
import { useNavigate } from "../../../router";
import { useActions, useApi, useDriver, useHost } from "../../../state";
import { Button, Card, Icon, Select, Skeleton } from "../../../ui";
import { KIND, provenance } from "../../config/provenance";
import { modelChoice } from "../../../state/domain";

/** Config tab: model and mode as the CLI reports them, and the effective config of this session with provenance. */
export function ConfigTab({ sessionKey, snap }: { sessionKey: string; snap: SessionSnapshot }) {
  const { host, id } = splitKey(sessionKey);
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const drv = useDriver(snap.driver);
  const hv = useHost(host);
  const [eff, setEff] = useState<EffectiveConfig | null>(null);
  const [error, setError] = useState("");
  const choice = modelChoice(snap);
  const current = choice.value ?? "";
  const [model, setModel] = useState(current);
  const [mode, setMode] = useState(snap.mode ?? "");
  useEffect(() => {
    let off = false;
    api.config.effective({ host, driver: snap.driver, project: snap.cwd ?? null, session: id }).then((e) => !off && setEff(e), (e) => !off && setError(errorMessage(e)));
    return () => {
      off = true;
    };
  }, [api, host, id, snap.driver, snap.cwd]);
  useEffect(() => setModel(current), [current]);
  useEffect(() => setMode(snap.mode ?? ""), [snap.mode]);
  const name = drv?.displayName ?? snap.driver;
  const live = ["idle", "working", "needs_you", "starting"].includes(snap.phase.state);
  const apply = () => {
    if (model && model !== current) void actions.command(sessionKey, { command: "set_model", model }, { failTitle: "Could not change the model" });
    if (mode && mode !== snap.mode) void actions.command(sessionKey, { command: "set_mode", mode }, { failTitle: "Could not change the mode" });
  };
  return (
    <div className="cfgcols">
      <Card>
        <div className="h3">Model and mode</div>
        <div className="field"><span className="lbl">Model</span>
          <Select label="Model" value={model || undefined} placeholder="Not reported" options={choice.options} onChange={setModel} />
          <span className="hint">{snap.available_models.length ? `Reported by ${name}${snap.cli_version ? ` ${snap.cli_version}` : ""} on ${hv?.name ?? host}: ${snap.available_models.map((m) => m.id).join(", ")}` : `${name} has not reported its models.`}</span>
        </div>
        {snap.command ? <div className="field"><span className="lbl">Command</span>
          <code className="mono">{snap.command}</code>
          <span className="hint">{name} runs as <b className="mono">{snap.command}</b> on {hv?.name ?? host}: the host's agent command when the session was created (a continued session keeps it).</span>
        </div> : null}
        <div className="field"><span className="lbl">Mode</span>
          <Select label="Mode" value={mode || undefined} placeholder="Not reported" options={snap.available_modes.map((m) => ({ value: m.id, label: m.label, description: m.description }))} onChange={setMode} />
          <span className="hint">{snap.available_modes.length ? `Modes offered by ${name}` : `${name} reports no modes.`}</span>
        </div>
        <div className="row-gap">
          <Button variant="primary" size="sm" disabled={!live || ((model === current || !snap.capabilities.set_model) && (mode === (snap.mode ?? "") || !snap.capabilities.set_mode))} onClick={apply}>Apply to running session</Button>
          {!snap.capabilities.set_model || !snap.capabilities.set_mode ? <span className="faint small">{name} cannot change {!snap.capabilities.set_model ? "the model" : "the mode"} without a restart.</span> : null}
        </div>
      </Card>
      <Card>
        <div className="h3 card-h">Effective tools and connectors<span className="sp" /><Button size="sm" variant="ghost" onClick={() => nav(`/config?driver=${encodeURIComponent(snap.driver)}&host=${encodeURIComponent(host)}&session=${encodeURIComponent(id)}`)}>Open Agent config <Icon name="chevr" size="xs" /></Button></div>
        {error ? <div className="c-failed small" role="alert">{error}</div> : !eff ? <><Skeleton /><Skeleton width="70%" /></> : (
          <table className="t"><tbody>
            {eff.items.map((it) => (
              <tr key={`${it.kind}:${it.name}`} className={it.active ? undefined : "faint"}>
                <td><span className="nmc"><Icon name={KIND[it.kind].icon} size="sm" /> {it.name}{it.locked ? <Icon name="lock" size="xs" className="faint" /> : null}</span>{it.unsupported ? <div className="reason">{it.unsupported}</div> : null}</td>
                <td className="r"><span className="provs">{provenance(it, name).map((b) => <span key={b.text} className={`prov ${b.cls}`}>{b.text}</span>)}</span></td>
              </tr>
            ))}
            {eff.items.length === 0 ? <tr><td className="faint">Nothing configured.</td></tr> : null}
          </tbody></table>
        )}
      </Card>
      <Card>
        <div className="h3">Environment and instructions</div>
        {!eff ? <Skeleton /> : (
          <table className="t"><tbody>
            {eff.env.map(([k, v, s]) => <tr key={k}><td className="mono">{k}={v}</td><td className="r faint">{s.scope}</td></tr>)}
            {eff.extra_instructions ? <tr><td>Extra instructions<div className="reason">{eff.extra_instructions.value}</div></td><td className="r faint">{eff.extra_instructions.scopes.map((s) => s.scope).join(", ")}</td></tr> : null}
            {eff.disallowed_tools.length ? <tr><td>Tools not allowed<div className="reason mono">{eff.disallowed_tools.join(", ")}</div></td><td /></tr> : null}
            {eff.unsupported.map((u) => <tr key={u.item}><td className="faint">{u.item}<div className="reason">{u.reason}</div></td><td className="r faint">not applied</td></tr>)}
            {!eff.env.length && !eff.extra_instructions && !eff.disallowed_tools.length && !eff.unsupported.length ? <tr><td className="faint">No environment, instructions or tool limits from Marsad.</td></tr> : null}
          </tbody></table>
        )}
      </Card>
    </div>
  );
}
