import { useEffect, useMemo, useState } from "react";
import type { DriverInventory, EnvironmentTest, HostEnvironment, HostEnvSettings, HostVar, SetupReport } from "../../bridge/wire";
import { useActions, useApi, useAppState, usePending } from "../../state";
import { Button, Checkbox, DriverBadge, Icon, IconButton, Wrap, makeDriver } from "../../ui";

const ENV_SOURCE: Record<HostEnvironment["source"], string> = {
  login_interactive_shell: "Your login shell (login + interactive, startup files included)",
  login_shell: "Your login shell (login only; the interactive start failed)",
  registry: "Your Windows user and system variables",
  process: "The helper's own environment (your shell could not be asked)",
};

function ago(ms: number, now: number): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  return s < 60 ? "just now" : s < 3600 ? `${Math.round(s / 60)} min ago` : `${Math.round(s / 3600)} h ago`;
}

/** One variable row being edited. `replace`: a stored secret whose value is being replaced. */
interface VarDraft extends HostVar {
  key: number;
  stored: boolean;
}

let keys = 0;
const draftOf = (s: HostEnvSettings) => ({
  setup: s.setup,
  vars: s.vars.map((v): VarDraft => ({ ...v, key: keys++, stored: v.secret && v.value == null })),
  commands: { ...s.commands } as Record<string, string>,
});
type Draft = ReturnType<typeof draftOf>;

const sameDraft = (a: Draft, b: Draft) =>
  a.setup === b.setup &&
  JSON.stringify(a.vars.map(({ name, value, secret }) => [name, value ?? null, secret])) === JSON.stringify(b.vars.map(({ name, value, secret }) => [name, value ?? null, secret])) &&
  JSON.stringify(Object.entries(a.commands).filter(([, c]) => c.trim()).sort()) === JSON.stringify(Object.entries(b.commands).filter(([, c]) => c.trim()).sort());

/**
 * The host's environment: what the login capture found, and the host's own settings applied on
 * top in order (setup commands → variables → agent commands), with Save and Test. Secret values
 * are write-only here: the hub never sends them back.
 */
export function EnvironmentSection({ hostId, hostName, windows, now }: { hostId: string; hostName: string; windows: boolean; now: number }) {
  const api = useApi();
  const actions = useActions();
  const inv = useAppState((s) => s.inventory[hostId]);
  const drivers = useAppState((s) => s.drivers);
  const [env, setEnv] = useState<HostEnvironment | null | undefined>(undefined);
  const [saved, setSaved] = useState<HostEnvSettings | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [notApplied, setNotApplied] = useState<string | null>(null);
  const [test, setTest] = useState<EnvironmentTest | null>(null);
  const refreshPk = `env:${hostId}`;
  const savePk = `envsave:${hostId}`;
  const testPk = `envtest:${hostId}`;
  const refreshing = !!usePending(refreshPk);
  const saving = !!usePending(savePk);
  const testing = !!usePending(testPk);

  useEffect(() => {
    let live = true;
    api.hostEnvironment(hostId).then((e) => live && setEnv(e), () => live && setEnv(null));
    api.hosts.environment(hostId).then(
      (s) => { if (live) { setSaved(s); setDraft(draftOf(s)); } },
      () => live && setSaved(null),
    );
    return () => { live = false; };
  }, [api, hostId]);

  const dirty = !!(saved && draft && !sameDraft(draft, draftOf(saved)));
  const missingSecret = draft?.vars.some((v) => v.secret && !v.stored && !v.value);
  const badName = draft?.vars.some((v) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(v.name));
  const applying = !!(saved && env && saved.revision && env.revision !== saved.revision && !notApplied);

  const refresh = () => void actions.run(refreshPk, `Could not refresh the environment of ${hostName}`, async () => setEnv(await api.refreshEnvironment(hostId)));
  const save = () => void actions.run(savePk, `Could not save the environment of ${hostName}`, async () => {
    if (!draft) return;
    const r = await api.hosts.setEnvironment({
      host: hostId,
      setup: draft.setup,
      vars: draft.vars.map(({ name, value, secret, stored }) => ({ name: name.trim(), secret, value: secret && stored ? null : value ?? "" })),
      commands: Object.fromEntries(Object.entries(draft.commands).map(([d, c]) => [d, c.trim()]).filter(([, c]) => c)),
      revision: "",
    });
    setSaved(r.settings);
    setDraft(draftOf(r.settings));
    setNotApplied(r.not_applied ?? null);
    if (r.environment) setEnv(r.environment);
    setTest(null);
  });
  const runTest = () => void actions.run(testPk, `Could not test the environment of ${hostName}`, async () => {
    const t = await api.hosts.testEnvironment(hostId);
    setTest(t);
    setEnv(t.environment);
  });

  const set = (f: (d: Draft) => Draft) => setDraft((d) => (d ? f(d) : d));
  const setVar = (key: number, patch: Partial<VarDraft>) => set((d) => ({ ...d, vars: d.vars.map((v) => (v.key === key ? { ...v, ...patch } : v)) }));
  const agentRows = useMemo(() => (inv ?? []).map((d) => ({ id: d.driver, name: d.display_name })), [inv]);
  const shellWord = windows ? "PowerShell" : env?.shell ? env.shell.split("/").pop() : "the login shell";

  return (
    <div className="envs">
      <h4 className="eyebrow">Environment</h4>
      {env === undefined ? <div className="faint small">Asking the host…</div> : env === null ? <div className="faint small">Not reported by this host.</div> : (
        <>
          <div className="small">{ENV_SOURCE[env.source]}{env.shell ? <> · <span className="mono">{env.shell}</span></> : null}</div>
          <div className="faint small">Captured {ago(env.captured_at_ms, now)} in {env.duration_ms} ms. Agents, terminals and git on {hostName} start with it{env.setup || env.variables.length || Object.keys(env.commands).length ? ", with the settings below applied" : ""}.</div>
          {env.notes.length ? <div className="gate bad mt6" role="note">{env.notes.map((n, i) => <div key={i} className="small mono"><Wrap text={n} /></div>)}</div> : null}
          {env.setup ? <SetupOutcome setup={env.setup} /> : null}
          <details className="mt6"><summary className="small">PATH ({env.path.length} entries)</summary><ol className="mono small" aria-label="PATH">{env.path.map((p, i) => <li key={i}><Wrap text={p} /></li>)}</ol></details>
        </>
      )}
      <Button size="sm" variant="ghost" icon="refresh" className="mt6" pending={refreshing} onClick={refresh}>Refresh environment</Button>

      {draft ? (
        <div className="envs-edit">
          <div className="field">
            <label htmlFor={`setup-${hostId}`}>Setup commands <span className="faint optl">optional</span></label>
            <textarea id={`setup-${hostId}`} className="ta mono" rows={3} spellCheck={false} value={draft.setup}
              placeholder={windows ? "$env:Path = \"C:\\tools\\bin;$env:Path\"" : "module load python/3.12\nexport PATH=\"$HOME/tools/bin:$PATH\""}
              onChange={(e) => set((d) => ({ ...d, setup: e.target.value }))} />
            <span className="hint">Run by {shellWord} after your login environment, before agents, terminals and git start (30 s at most). What they leave in the environment is what those get.</span>
          </div>

          <div className="field">
            <label id={`vars-${hostId}`}>Variables</label>
            {draft.vars.length ? (
              <div className="envs-vars" role="table" aria-labelledby={`vars-${hostId}`}>
                {draft.vars.map((v) => (
                  <div key={v.key} className="envs-var" role="row">
                    <div className="input" role="cell"><input aria-label="Variable name" className="mono" spellCheck={false} placeholder="NAME" value={v.name} onChange={(e) => setVar(v.key, { name: e.target.value })} /></div>
                    <div className="input" role="cell">
                      {v.secret && v.stored
                        ? <input aria-label={`Value of ${v.name || "the variable"}`} className="mono" value="••••••••" readOnly tabIndex={-1} />
                        : <input aria-label={`Value of ${v.name || "the variable"}`} className="mono" spellCheck={false} autoComplete="off" type={v.secret ? "password" : "text"} placeholder={v.secret ? "secret value" : "value"} value={v.value ?? ""} onChange={(e) => setVar(v.key, { value: e.target.value })} />}
                    </div>
                    <div role="cell" className="envs-var-acts">
                      {v.secret && v.stored ? <Button size="sm" variant="ghost" onClick={() => setVar(v.key, { stored: false, value: "" })}>Replace</Button> : null}
                      <Checkbox label="Secret" checked={v.secret} onChange={(c) => setVar(v.key, c ? { secret: true, stored: false, value: v.value ?? "" } : { secret: false, stored: false, value: v.stored ? "" : v.value ?? "" })} />
                      <IconButton icon="x" label={`Remove ${v.name || "variable"}`} onClick={() => set((d) => ({ ...d, vars: d.vars.filter((x) => x.key !== v.key) }))} />
                    </div>
                  </div>
                ))}
              </div>
            ) : null}
            <div><Button size="sm" variant="ghost" icon="plus" onClick={() => set((d) => ({ ...d, vars: [...d.vars, { key: keys++, name: "", value: "", secret: false, stored: false }] }))}>Add variable</Button></div>
            <span className="hint">Set after the setup commands, as written (no expansion). A secret is never shown again, logged or sent back here.</span>
          </div>

          <div className="field">
            <label id={`cmds-${hostId}`}>Agent commands <span className="faint optl">optional</span></label>
            {agentRows.length ? (
              <table className="t envs-cmds" aria-labelledby={`cmds-${hostId}`}><tbody>
                {agentRows.map((a) => (
                  <tr key={a.id}>
                    <td><DriverBadge driver={drivers[a.id] ?? makeDriver(a.id, a.name)} /></td>
                    <td><div className="input"><input aria-label={`Command for ${a.name}`} className="mono" spellCheck={false} placeholder={a.id} value={draft.commands[a.id] ?? ""} onChange={(e) => set((d) => ({ ...d, commands: { ...d.commands, [a.id]: e.target.value } }))} /></div></td>
                  </tr>
                ))}
              </tbody></table>
            ) : <div className="faint small">Known once the host reports its agents.</div>}
            <span className="hint">A name on PATH or a path that stands in for the agent with the same arguments (for example a site wrapper). Marsad checks that it answers like the agent.</span>
          </div>

          <div className="acts">
            <Button size="sm" variant="primary" icon="check" pending={saving} disabled={!dirty || !!missingSecret || !!badName || testing} onClick={save}>Save</Button>
            <Button size="sm" icon="sparkle" pending={testing} disabled={dirty || saving} onClick={runTest} title={dirty ? "Save first: Test runs the saved settings" : undefined}>Test</Button>
            <span className="faint small">{missingSecret ? "Enter a value for each new secret." : badName ? "Names are letters, digits and '_', not starting with a digit." : "Applies to new sessions and terminals; running ones keep their environment."}</span>
          </div>
          {notApplied ? <div className="gate mt6" role="status"><div className="gate-h"><Icon name="clock" size="sm" />Saved, not applied yet</div><div className="small"><Wrap text={notApplied} /></div><div className="faint small">Marsad applies them when {hostName} connects.</div></div> : null}
          {applying ? <div className="faint small" role="status"><Icon name="working" size="xs" spin /> Applying on {hostName}…</div> : null}
          {testing ? <div className="faint small" role="status">Running the setup and detecting agents on {hostName}; this can take up to a minute.</div> : null}
          {test ? <TestResult test={test} /> : null}
        </div>
      ) : saved === null ? <div className="faint small mt6">The settings could not be read.</div> : null}
    </div>
  );
}

/** How the setup commands went: plainly failed (status, reason, output), or a quiet success. */
function SetupOutcome({ setup }: { setup: SetupReport }) {
  if (!setup.ok) {
    return (
      <div className="gate bad mt6" role="alert">
        <div className="gate-h"><Icon name="failed" size="sm" />Setup commands failed</div>
        <div className="small"><Wrap text={setup.error ?? "They did not finish."} />{setup.exit_code != null ? ` (exit status ${setup.exit_code})` : ""}</div>
        <div className="faint small">{setup.applied ? "The environment they left is used anyway." : "Agents, terminals and git get your login environment without them."} Ran by <span className="mono">{setup.shell}</span> in {setup.duration_ms} ms.</div>
        {setup.output ? <pre className="envs-out mono" aria-label="Setup output">{setup.output}</pre> : null}
      </div>
    );
  }
  return (
    <details className="mt6">
      <summary className="small"><span className="c-done"><Icon name="done" size="xs" /></span> Setup commands ran in {setup.duration_ms} ms</summary>
      <div className="faint small">By <span className="mono">{setup.shell}</span>.</div>
      {setup.output ? <pre className="envs-out mono" aria-label="Setup output">{setup.output}</pre> : <div className="faint small">No output.</div>}
    </details>
  );
}

function agentLine(d: DriverInventory): { ok: boolean; text: string } {
  if (d.refreshing) return { ok: false, text: "Still detecting…" };
  if (!d.installed) return { ok: false, text: d.error ?? "Not found" };
  return { ok: !d.warning && !d.error, text: d.version ?? d.error ?? "found" };
}

/** What a Test found: per agent the command, where it resolved, its version or why not. */
function TestResult({ test }: { test: EnvironmentTest }) {
  const drivers = useAppState((s) => s.drivers);
  return (
    <div className="envs-test" aria-label="Test result">
      <div className="small"><b>Test result</b>{test.complete ? "" : " (some agents were still being detected)"}</div>
      <table className="t"><tbody>
        {test.agents.map((d) => {
          const l = agentLine(d);
          return (
            <tr key={d.driver}>
              <td><DriverBadge driver={drivers[d.driver] ?? makeDriver(d.driver, d.display_name)} /></td>
              <td>
                <div className={l.ok ? "mono" : d.installed ? "mono c-needs" : "faint"}><Wrap text={l.text} /></div>
                <div className="faint small mono"><Wrap text={[d.command ? `${d.command} →` : null, d.binary_path].filter(Boolean).join(" ")} /></div>
                {d.warning ? <div className="small c-needs"><Icon name="warn" size="xs" /> <Wrap text={d.warning} /></div> : null}
              </td>
            </tr>
          );
        })}
      </tbody></table>
    </div>
  );
}
