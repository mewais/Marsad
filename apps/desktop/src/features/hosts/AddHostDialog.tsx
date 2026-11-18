import { useEffect, useState } from "react";
import type { HostKind, ImportSuggestions, SshConfigHost, WslDistros } from "../../bridge/wire";
import { linkView, useActions, useApi, useAppState, usePending } from "../../state";
import { Button, Dialog, Icon, Segmented, TextField } from "../../ui";
import { WslDistroPicker, defaultDistro } from "./WslDistroPicker";

interface Draft {
  kind: "ssh" | "wsl";
  name: string;
  target: string;
  port: string;
  identity: string;
  options: string;
  distro: string;
}
const empty: Draft = { kind: "ssh", name: "", target: "", port: "", identity: "", options: "", distro: "" };

export function hostKindOf(d: Draft): HostKind {
  if (d.kind === "wsl") return { kind: "wsl", distro: d.distro.trim() };
  return { kind: "ssh", target: d.target.trim(), port: d.port.trim() ? Number(d.port) : null, identity_file: d.identity.trim() || null, options: d.options.trim() ? d.options.trim().split(/\s+/) : [] };
}

/** Add host: SSH (alias, user@host, port, key, options) or WSL; suggestions from ~/.ssh/config; then live progress from the fleet. */
export function AddHostDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const api = useApi();
  const actions = useActions();
  const [d, setD] = useState<Draft>(empty);
  const [sugg, setSugg] = useState<ImportSuggestions | null>(null);
  const [wsl, setWsl] = useState<WslDistros | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const pending = !!usePending("hosts.add");
  const set = (p: Partial<Draft>) => setD((x) => ({ ...x, ...p }));
  useEffect(() => {
    if (!open) return;
    setD(empty);
    setAdded(null);
    api.hosts.importSuggestions().then(setSugg, () => setSugg(null));
    setWsl(null);
    api.hosts.wslDistros().then(setWsl, () => setWsl({ available: false, distros: [] }));
  }, [open, api]);
  // WSL's default distribution is preselected once the list arrives.
  useEffect(() => {
    if (wsl) setD((x) => (x.distro ? x : { ...x, distro: defaultDistro(wsl) }));
  }, [wsl]);
  const host = useAppState((s) => (added ? s.hosts[added] : undefined));
  const inv = useAppState((s) => (added ? s.inventory[added] : undefined));

  const useSugg = (h: SshConfigHost) => set({ kind: "ssh", name: h.alias, target: h.alias, port: h.port ? String(h.port) : "", identity: h.identity_file ?? "" });
  const problem = !d.name.trim() ? "Give the host a name." : d.kind === "ssh" && !d.target.trim() ? "Enter an ssh alias or user@host." : d.kind === "wsl" && !d.distro.trim() ? "Pick the WSL distribution." : d.port && !/^\d+$/.test(d.port) ? "The port is a number." : "";
  const submit = async () => {
    if (problem) return;
    const e = await actions.run("hosts.add", "Could not add the host", () => api.hosts.add({ name: d.name.trim(), kind: hostKindOf(d), id: null }));
    if (e) setAdded(e.id);
  };

  const lv = host ? linkView(host.link, host.authPrompt) : null;
  const st = host?.link.link === "remote" ? host.link.state.state : host ? "connected" : null;
  const step = (n: number, label: string, cur: number) => <span className={`s${cur > n ? " dn" : cur === n ? " on" : ""}`}><b>{cur > n ? <Icon name="check" size="xs" /> : n}</b>{label}</span>;
  const cur = !added ? 1 : st === "connecting" ? 1 : st === "provisioning" ? 2 : st === "connected" ? (inv ? 4 : 3) : 1;

  return (
    <Dialog open={open} onClose={onClose} title="Add host" width={640}
      footer={added ? (
        <><span className="faint small">{lv?.detail}</span><span className="sp" /><Button variant="primary" onClick={onClose}>{cur >= 4 ? "Done" : "Close and keep going"}</Button></>
      ) : (
        <><span className="faint small">{problem}</span><span className="sp" /><Button onClick={onClose}>Cancel</Button><Button variant="primary" disabled={!!problem} pending={pending} onClick={() => void submit()}>Add and connect</Button></>
      )}>
      <div className="addh">
        <div className="steps">{step(1, "Connect", cur)}<Icon name="chevr" size="xs" />{step(2, "Install helper", cur)}<Icon name="chevr" size="xs" />{step(3, "Detect agents", cur)}</div>
        {!added ? (
          <>
            {wsl?.available ? <Segmented label="Connection" value={d.kind} onChange={(kind) => set({ kind, ...(kind === "wsl" && !d.name.trim() && d.distro ? { name: d.distro } : {}) })} options={[{ value: "ssh", label: "SSH" }, { value: "wsl", label: "WSL" }]} /> : null}
            <TextField label="Name" value={d.name} onChange={(e) => set({ name: e.target.value })} placeholder="build-box" />
            {d.kind === "ssh" ? (
              <>
                <div className="two">
                  <TextField label="SSH target" hint="An alias from ~/.ssh/config or user@host" value={d.target} onChange={(e) => set({ target: e.target.value })} placeholder="ops@ci-runner-02.internal" spellCheck={false} />
                  <TextField label="Port" value={d.port} onChange={(e) => set({ port: e.target.value })} placeholder="22" inputMode="numeric" />
                </div>
                <div className="two">
                  <TextField label="Identity file" value={d.identity} onChange={(e) => set({ identity: e.target.value })} placeholder="~/.ssh/id_ed25519" spellCheck={false} />
                  <TextField label="Extra ssh options" hint="Space-separated -o values" value={d.options} onChange={(e) => set({ options: e.target.value })} placeholder="ProxyJump=bastion" spellCheck={false} />
                </div>
                {sugg?.hosts.length ? (
                  <div className="field">
                    <span className="lbl">From {sugg.source ?? "~/.ssh/config"}</span>
                    <div className="sugg">
                      {sugg.hosts.map((h) => (
                        <button key={h.alias} type="button" className="chip" disabled={h.already_added} onClick={() => useSugg(h)} title={h.already_added ? "Already added" : `${h.user ? `${h.user}@` : ""}${h.hostname ?? h.alias}`}>
                          <Icon name="server" size="xs" />{h.alias}{h.already_added ? <span className="faint"> · added</span> : null}
                        </button>
                      ))}
                    </div>
                  </div>
                ) : null}
              </>
            ) : <WslDistroPicker list={wsl} value={d.distro} onChange={(distro) => set({ distro })} />}
            <p className="faint small">Marsad installs a small helper as your user on the host. It keeps sessions alive when Marsad disconnects and never opens a port.</p>
          </>
        ) : (
          <div className="card addh-prog" role="status" aria-live="polite">
            <div className="h3">{d.name}</div>
            <div className="check">
              <div className={cur > 1 ? "" : "c-working"}><Icon name={cur > 1 ? "done" : "working"} size="sm" spin={cur === 1} className={cur > 1 ? "c-done" : ""} />{cur > 1 ? "Connected" : lv?.detail ?? "Connecting…"}</div>
              <div className={cur === 2 ? "c-working" : cur < 2 ? "faint" : ""}><Icon name={cur > 2 ? "done" : cur === 2 ? "working" : "idle"} size="sm" spin={cur === 2} className={cur > 2 ? "c-done" : ""} />{cur === 2 ? lv?.detail : "Install the Marsad helper"}</div>
              <div className={cur === 3 ? "c-working" : cur < 3 ? "faint" : ""}><Icon name={cur > 3 ? "done" : cur === 3 ? "working" : "idle"} size="sm" spin={cur === 3} className={cur > 3 ? "c-done" : ""} />{inv ? `Found ${inv.filter((x) => x.installed).map((x) => `${x.display_name} ${x.version ?? ""}`).join(", ") || "no agent CLIs"}` : "Scan PATH for agent CLIs"}</div>
            </div>
            {lv && !lv.connected && lv.tone === "bad" ? <div className="c-failed small" role="alert">{lv.detail}</div> : null}
          </div>
        )}
      </div>
    </Dialog>
  );
}
