import { useState } from "react";
import { useNavigate, sessionPath } from "../../router";
import { linkView, osWord, useActions, useSessionView, useApi, useAppState, usePending, type HostView } from "../../state";
import { Button, ConfirmDialog, ConnState, DriverBadge, Icon, STATUS, hostIcon, makeDriver, useNow } from "../../ui";
import { address, linkSub } from "./linkText";

/** Right panel of the Hosts screen: link state with the action it calls for, connection, CLIs, sessions. */
export function HostDetail({ host }: { host: HostView }) {
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const entry = useAppState((s) => s.hosts[host.id]?.entry);
  const inv = useAppState((s) => s.inventory[host.id]);
  const keys = useAppState((s) => Object.keys(s.sessions).filter((k) => k.startsWith(host.id + "/")).join("|"));
  const drivers = useAppState((s) => s.drivers);
  const [confirm, setConfirm] = useState(false);
  const reconnectPk = `reconnect:${host.id}`;
  const busy = !!usePending(reconnectPk);
  const now = useNow(true);
  const lv = linkView(host.link, host.authPrompt, now);
  const st = host.link.link === "remote" ? host.link.state : null;
  const reconnect = () => void actions.run(reconnectPk, `Could not connect to ${host.name}`, () => api.hosts.reconnect(host.id));
  const remove = async () => {
    const ok = await actions.run(`remove:${host.id}`, `Could not remove ${host.name}`, () => api.hosts.remove(host.id));
    setConfirm(false);
    if (ok) nav("/hosts");
  };
  const k = entry?.kind;

  let card: React.ReactNode = null;
  if (host.authPrompt) {
    card = <div className="gate"><div className="gate-h"><Icon name="key" size="sm" />Waiting for you to sign in</div><div className="small">{host.name} asked for {host.authPrompt.kind === "secret" ? "a password" : host.authPrompt.kind === "code" ? "a one-time code" : host.authPrompt.kind === "confirm" ? "a yes or no" : "approval"}. The sign-in dialog is open.</div></div>;
  } else if (st?.state === "auth_needed") {
    card = (
      <div className="gate bad"><div className="gate-h"><Icon name="key" size="sm" />{lv.word}</div><div className="small">{st.detail}</div>
        <div className="acts"><Button size="sm" variant="primary" icon="key" pending={busy} onClick={reconnect}>Connect</Button><span className="faint small">You may be asked for a password or code.</span></div></div>
    );
  } else if (st?.state === "reconnecting") {
    card = (
      <div className="gate"><div className="gate-h"><Icon name="refresh" size="sm" />{lv.detail}</div><div className="small">The connection dropped ({st.last_error}). Sessions keep running on the machine; Marsad catches up on their output when it reconnects. Attempt {st.attempt}.</div>
        <div className="acts"><Button size="sm" icon="refresh" pending={busy} onClick={reconnect}>Retry now</Button></div></div>
    );
  } else if (st?.state === "provisioning" || st?.state === "connecting") {
    card = (
      <div className="gate info"><div className="gate-h"><Icon name="working" size="sm" spin />{lv.detail}</div>
        <div className="small">{st.state === "provisioning" ? (st.reason.kind === "build_mismatch" ? `Updating the Marsad helper from ${st.reason.found} to ${st.reason.expected}.` : st.reason.kind === "command_not_found" ? "Installing the Marsad helper on this host for the first time." : "Updating the helper to a compatible protocol.") : "Opening the connection."}</div>
        <div className="bar w indet" aria-hidden="true"><i /></div></div>
    );
  } else if (st && (st.state === "failed" || st.state === "incompatible" || st.state === "stopped")) {
    card = <div className="gate bad"><div className="gate-h"><Icon name="failed" size="sm" />{lv.word}</div><div className="small">{lv.detail}</div><div className="acts"><Button size="sm" icon="refresh" pending={busy} onClick={reconnect}>Connect again</Button></div></div>;
  }

  return (
    <aside className="hd" aria-label={`${host.name} details`}>
      <div className="hd-title"><Icon name={hostIcon(host)} size="lg" /><div><div className="h2">{host.name}</div><div className="faint small">{[osWord(entry?.platform?.os), entry?.platform?.arch, host.local ? "Local" : k?.kind === "ssh" ? "SSH" : "WSL"].filter(Boolean).join(" · ")}</div></div><span className="sp" /><ConnState host={host} /></div>
      {card}
      <div>
        <h4 className="eyebrow">Connection</h4>
        <dl className="kvl">
          <dt>Address</dt><dd className="mono">{entry ? address(entry) : ""}</dd>
          {k?.kind === "ssh" ? <><dt>Key</dt><dd className="mono">{k.identity_file ?? "ssh default"}</dd>{k.options?.length ? <><dt>Options</dt><dd className="mono">{k.options.join(" ")}</dd></> : null}</> : null}
          <dt>Platform</dt><dd className="mono">{entry?.platform?.target ?? "not probed yet"}</dd>
          <dt>Status</dt><dd>{linkSub(host.link, now)}</dd>
          <dt>Sign-in</dt><dd>{host.interactiveAuth ? "Interactive (asks each time)" : host.local ? "Not needed" : "Key or agent"}</dd>
        </dl>
      </div>
      <div>
        <h4 className="eyebrow">Agent CLIs</h4>
        {!inv ? <div className="faint small">{lv.connected ? "Asking the host…" : "Known once the host is connected."}</div> : (
          <table className="t"><tbody>
            {inv.map((d) => (
              <tr key={d.driver}><td><DriverBadge driver={drivers[d.driver] ?? makeDriver(d.driver, d.display_name)} /></td><td className={d.installed ? "mono" : "faint"}>{d.installed ? d.version ?? "installed" : "Not installed"}</td><td className="r faint small" title={d.binary_path ?? d.error ?? ""}>{d.installed ? d.binary_path : d.error}</td></tr>
            ))}
          </tbody></table>
        )}
        {lv.connected ? <Button size="sm" variant="ghost" icon="refresh" className="mt6" onClick={() => void actions.run(`refresh:${host.id}`, "Could not refresh the agents", () => api.refreshDrivers(host.id))}>Re-detect agents</Button> : null}
      </div>
      <div>
        <h4 className="eyebrow">Sessions here</h4>
        <div className="hist">
          {keys ? keys.split("|").map((key) => <SessionLine key={key} sessionKey={key} />) : <div className="faint small">No sessions on this host.</div>}
        </div>
      </div>
      {!host.local ? <div className="hd-foot"><Button size="sm" variant="ghost" icon="x" onClick={() => setConfirm(true)}>Remove host</Button></div> : null}
      <ConfirmDialog open={confirm} title={`Remove ${host.name}?`} confirmLabel="Remove host" danger onConfirm={() => void remove()} onClose={() => setConfirm(false)}>
        <p>Marsad stops connecting to {host.name}. Sessions already running there keep running; you can add the host again later.</p>
      </ConfirmDialog>
    </aside>
  );
}

function SessionLine({ sessionKey }: { sessionKey: string }) {
  const v = useSessionView(sessionKey);
  const nav = useNavigate();
  if (!v) return null;
  return (
    <button type="button" className="h hbtn" onClick={() => nav(sessionPath(v.host, v.id))}>
      <span className={`c-${v.status}`}><Icon name={STATUS[v.status].icon} size="sm" spin={v.status === "working"} /></span><span className="ell">{v.name}</span><span className={`w c-${v.status}`}>{STATUS[v.status].label}</span>
    </button>
  );
}
