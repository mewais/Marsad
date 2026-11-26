import { useEffect, useState } from "react";
import type { WslDistros } from "../../bridge/wire";
import { useNavigate, sessionPath } from "../../router";
import { askWords, backgroundWords, driverVersionState, droppedWords, keepRunningWords, linkView, NO_RETRY_WORDS, osWord, reconnectPendingKey, sameMachineAs, signOutPendingKey, useActions, useDeveloperMode, useReach, useSessionView, useApi, useAppState, usePending, useSignInQueue, type HostView } from "../../state";
import { Button, ConfirmDialog, ConnState, ConnectingWords, DriverBadge, ErrorBoundary, Icon, STATUS, Wrap, hostIcon, makeDriver, useNow } from "../../ui";
import { address, feedReason, linkSub } from "./linkText";
import { SignInButton } from "./SignInButton";
import { WslDistroPicker, defaultDistro } from "./WslDistroPicker";
import { EnvironmentSection } from "./EnvironmentSection";
import { HostDiagnostics } from "../diagnostics/HostDiagnostics";

/** Right panel of the Hosts screen: link state with the action it calls for, connection, CLIs, sessions. */
export function HostDetail({ host }: { host: HostView }) {
  const api = useApi();
  const actions = useActions();
  const nav = useNavigate();
  const entry = useAppState((s) => s.hosts[host.id]?.entry);
  const inv = useAppState((s) => s.inventory[host.id]);
  const keys = useAppState((s) => Object.keys(s.sessions).filter((k) => k.startsWith(host.id + "/")).join("|"));
  const drivers = useAppState((s) => s.drivers);
  const twins = useAppState((s) => sameMachineAs(s.hosts, host.id).join(", "));
  const [confirm, setConfirm] = useState(false);
  const [confirmSignOut, setConfirmSignOut] = useState(false);
  const busy = !!usePending(reconnectPendingKey(host.id));
  const signingOut = !!usePending(signOutPendingKey(host.id));
  const queue = useSignInQueue();
  const now = useNow(true);
  const lv = linkView(host.link, host.authPrompt, now);
  const st = host.link.link === "remote" ? host.link.state : null;
  const reconnect = () => void actions.reconnectHost(host.id, host.name);
  const reach = useReach();
  const developer = useDeveloperMode();
  // The local host is reached through the app's own link to the local daemon, not through the hub.
  const hub = host.link.link === "local" ? host.link.hub : undefined;
  const remove = async () => {
    const ok = await actions.run(`remove:${host.id}`, `Could not remove ${host.name}`, () => api.hosts.remove(host.id));
    setConfirm(false);
    if (ok) nav("/hosts");
  };
  const k = entry?.kind;

  let card: React.ReactNode = null;
  if (hub && hub.state !== "connected") {
    card = (
      <div className={`gate ${hub.state === "offline" ? "bad" : hub.state === "connecting" ? "info" : ""}`}>
        <div className="gate-h"><Icon name={hub.state === "connecting" ? "working" : hub.state === "offline" ? "failed" : "refresh"} size="sm" spin={hub.state === "connecting"} />{hub.state === "connecting" ? "Connecting to the local service" : hub.state === "reconnecting" ? "Lost the local service; reconnecting" : "Cannot reach the local service"}</div>
        <div className="small">{hub.error ? `${hub.error}. ` : ""}Everything Marsad shows is the last known state until the service is back.{hub.state === "offline" ? " Marsad does not retry this on its own." : ""}</div>
        {hub.state === "connecting" ? null : <div className="acts"><Button size="sm" icon="refresh" onClick={() => void actions.reconnectHub()}>{hub.state === "offline" ? "Try again" : "Try again now"}</Button></div>}
      </div>
    );
  } else if (host.authPrompt) {
    const ahead = queue.findIndex((q) => q.prompt.host === host.id);
    card = (
      <div className="gate"><div className="gate-h"><Icon name="key" size="sm" />{host.name} has a question for you</div>
        <div className="small">{host.name} asked for {askWords(host.authPrompt.kind)}. {ahead <= 0 ? "Answer it in the sign-in dialog." : `It is asked after ${ahead === 1 ? "one other question" : `${ahead} other questions`} in the sign-in dialog.`}</div></div>
    );
  } else if (st?.state === "auth_needed" && st.held) {
    card = (
      <div className="gate" data-testid="host-held"><div className="gate-h"><Icon name="key" size="sm" />Needs you to sign in</div>
        <div className="small">{droppedWords(host.name, st.cause)} {NO_RETRY_WORDS}</div>
        {host.sharedConnection
          ? <>
              <div className="small" data-testid="held-detail">{st.detail.replace(/[.\s]+$/, "").replace(/^./, (c) => c.toUpperCase())}.</div>
              <div className="small faint">Signing in asks you for something (a password, a code or an approval on your phone), so it waits for your click. Once you have signed in, Marsad keeps the connection open and reconnects without asking until you sign out or the connection ends.</div>
            </>
          : <div className="small faint">Every attempt asks you for something (a password, a code or an approval on your phone), so it waits for your click.</div>}
        <div className="acts"><SignInButton host={host} /></div></div>
    );
  } else if (st?.state === "auth_needed") {
    card = (
      <div className="gate bad"><div className="gate-h"><Icon name="key" size="sm" />{lv.word}</div><div className="small">{st.detail}</div>
        <div className="acts"><Button size="sm" variant="primary" icon="key" pending={busy} blocked={reach} onClick={reconnect}>Connect</Button><span className="faint small">You may be asked for a password or code.</span></div></div>
    );
  } else if (st?.state === "reconnecting") {
    card = (
      <div className="gate"><div className="gate-h"><Icon name="refresh" size="sm" />{lv.detail}</div><div className="small">The connection dropped ({st.last_error}). Sessions keep running on the machine; Marsad catches up on their output when it reconnects. Attempt {st.attempt}.</div>
        <div className="acts"><Button size="sm" icon="refresh" pending={busy} blocked={reach} onClick={reconnect}>Retry now</Button></div></div>
    );
  } else if (st?.state === "provisioning" || st?.state === "connecting") {
    card = (
      <div className="gate info"><div className="gate-h"><Icon name="working" size="sm" spin />{st.state === "connecting" ? <ConnectingWords stage={st.stage} since={st.since_ms} /> : lv.detail}</div>
        <div className="small">{st.state === "provisioning" ? (st.reason.kind === "build_mismatch" ? `Updating the Marsad helper from ${st.reason.found} to ${st.reason.expected}.` : st.reason.kind === "command_not_found" ? "Installing the Marsad helper on this host for the first time." : "Updating the helper to a compatible protocol.") : "Opening the connection."}</div>
        <div className="bar w indet" aria-hidden="true"><i /></div></div>
    );
  } else if (st?.state === "failed" && st.problem === "unknown_distro" && entry?.kind.kind === "wsl") {
    card = <div className="gate bad"><div className="gate-h"><Icon name="failed" size="sm" />{lv.word}</div><div className="small">{lv.detail}</div><FixDistro hostId={host.id} hostName={host.name} /></div>;
  } else if (st && (st.state === "failed" || st.state === "incompatible" || st.state === "stopped")) {
    card = <div className="gate bad"><div className="gate-h"><Icon name="failed" size="sm" />{lv.word}</div><div className="small">{lv.detail}</div><div className="acts"><Button size="sm" icon="refresh" pending={busy} blocked={reach} onClick={reconnect}>Try again</Button>{st.state === "failed" ? <span className="faint small">Marsad does not retry this on its own.</span> : null}</div></div>;
  }

  return (
    <aside className="hd" aria-label={`${host.name} details`}>
      <div className="hd-title"><Icon name={hostIcon(host)} size="lg" /><div><div className="h2">{host.name}</div><div className="faint small">{[osWord(entry?.platform?.os), entry?.platform?.arch, host.local ? "Local" : k?.kind === "ssh" ? "SSH" : "WSL"].filter(Boolean).join(" · ")}</div></div><span className="sp" /><ConnState host={host} /></div>
      {card}
      {host.feedError ? <div className="small" role="note" data-testid="host-feed-error">Marsad stopped receiving this host's session list: {feedReason(host.feedError)}. It will resume when the host reconnects.</div> : null}
      {twins ? <div className="gate" role="note"><div className="gate-h"><Icon name="warn" size="sm" />Same machine as {twins}</div><div className="small">These hosts reach one machine. Keep one of them: sessions and agents are the same on both.</div></div> : null}
      <div>
        <h4 className="eyebrow">Connection</h4>
        <dl className="kvl">
          <dt>Address</dt><dd className="mono"><Wrap text={entry ? address(entry) : ""} /></dd>
          {k?.kind === "ssh" ? <><dt>Key</dt><dd className="mono"><Wrap text={k.identity_file ?? "ssh default"} /></dd>{k.options?.length ? <><dt>Options</dt><dd className="mono"><Wrap text={k.options.join(" ")} /></dd></> : null}</> : null}
          <dt>Platform</dt><dd className="mono">{entry?.platform?.target ?? "not probed yet"}</dd>
          <dt>Status</dt><dd>{linkSub(host.link, now)}</dd>
          <dt>Sign-in</dt><dd>{host.interactiveAuth || host.authPrompt ? (host.sharedConnection ? "Interactive (asks when it signs in; the connection then stays open)" : "Interactive (asks each time)") : host.local ? "Not needed" : "Key or agent"}</dd>
          {host.sharedConnection && lv.connected ? <><dt>Connection</dt><dd>Kept open and shared, until you sign out or it ends</dd></> : null}
        </dl>
      </div>
      <div>
        <h4 className="eyebrow">Agent CLIs</h4>
        {!inv ? <div className="faint small">{lv.connected ? "Asking the host…" : "Known once the host is connected."}</div> : (
          <table className="t"><tbody>
            {inv.map((d) => {
              const vs = driverVersionState(d, lv.connected);
              const versionText = vs.kind === "ready" ? vs.version : vs.kind === "loading" ? "loading…" : vs.kind === "not_reported" ? "Not reported" : "Not installed";
              const sub = vs.kind === "loading" ? (d.command ? `${d.command} →` : "") : [d.command ? `${d.command} →` : null, (d.installed ? d.binary_path : d.error) ?? ""].filter(Boolean).join(" ");
              return (
                <tr key={d.driver}>
                  <td><DriverBadge driver={drivers[d.driver] ?? makeDriver(d.driver, d.display_name)} /></td>
                  <td>
                    <div className={vs.kind === "ready" ? "mono" : "faint"}><Wrap text={versionText} /></div>
                    {sub ? <div className="faint small"><Wrap text={sub} /></div> : null}
                    {d.warning ? <div className="small c-needs"><Icon name="warn" size="xs" /> <Wrap text={d.warning} /></div> : null}
                  </td>
                </tr>
              );
            })}
          </tbody></table>
        )}
        {lv.connected ? <Button size="sm" variant="ghost" icon="refresh" className="mt6" blocked={reach} onClick={() => void actions.run(`refresh:${host.id}`, "Could not refresh the agents", () => api.refreshDrivers(host.id))}>Re-detect agents</Button> : null}
      </div>
      {lv.connected ? <EnvironmentSection hostId={host.id} hostName={host.name} windows={entry?.platform?.os === "windows"} now={now} /> : null}
      <div>
        <h4 className="eyebrow">Sessions here</h4>
        {st?.state === "auth_needed" && st.held && keys ? <div className="small faint" data-testid="held-sessions">{keepRunningWords(keys.split("|").length)}</div> : null}
        <div className="hist">
          {keys ? keys.split("|").map((key) => <SessionLine key={key} sessionKey={key} />) : <div className="faint small">No sessions on this host.</div>}
        </div>
      </div>
      {developer ? <ErrorBoundary variant="item" name="host_diagnostics" what="the diagnostics" resetKey={host.id}><HostDiagnostics host={host} /></ErrorBoundary> : null}
      {!host.local ? (
        <div className="hd-foot">
          {host.sharedConnection && lv.connected ? <Button size="sm" variant="ghost" icon="key" pending={signingOut} blocked={reach} onClick={() => setConfirmSignOut(true)}>Sign out</Button> : null}
          <Button size="sm" variant="ghost" icon="x" blocked={reach} onClick={() => setConfirm(true)}>Remove host</Button>
        </div>
      ) : null}
      <ConfirmDialog open={confirmSignOut} title={`Sign out of ${host.name}?`} confirmLabel="Sign out" onConfirm={() => { setConfirmSignOut(false); void actions.signOutHost(host.id, host.name); }} onClose={() => setConfirmSignOut(false)}>
        <p>Marsad closes the connection it keeps open to {host.name}. Sessions already running there keep running, and Marsad will not reconnect until you sign in again.</p>
      </ConfirmDialog>
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
      <span className={v.stale ? "faint" : `c-${v.status}`}><Icon name={STATUS[v.status].icon} size="sm" spin={v.status === "working" && !v.stale} /></span><span className="ell">{v.name}</span><span className={v.stale ? "w lk" : `w c-${v.status}`} title={v.backgroundOnly && !v.stale ? backgroundWords(v.backgroundOnly) : undefined}>{STATUS[v.status].label}{v.stale ? " · last known" : ""}</span>
    </button>
  );
}

/** A WSL host whose distribution is gone: pick an installed one and reconnect. */
function FixDistro({ hostId, hostName }: { hostId: string; hostName: string }) {
  const api = useApi();
  const actions = useActions();
  const entry = useAppState((s) => s.hosts[hostId]?.entry);
  const [list, setList] = useState<WslDistros | null>(null);
  const [distro, setDistro] = useState("");
  const pk = `fixdistro:${hostId}`;
  const busy = !!usePending(pk);
  useEffect(() => {
    api.hosts.wslDistros().then((l) => { setList(l); setDistro(defaultDistro(l)); }, () => setList({ available: false, distros: [] }));
  }, [api]);
  if (!entry || entry.kind.kind !== "wsl") return null;
  const save = () => void actions.run(pk, `Could not change ${hostName}`, async () => {
    await api.hosts.update({ ...entry, kind: { kind: "wsl", distro: distro.trim() } });
    return api.hosts.reconnect(hostId);
  });
  return (
    <>
      <WslDistroPicker list={list} value={distro} onChange={setDistro} />
      <div className="acts"><Button size="sm" variant="primary" icon="refresh" pending={busy} disabled={!distro.trim()} onClick={save}>Use this distribution</Button></div>
    </>
  );
}
