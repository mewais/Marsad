import type { DriverInventory, EnvironmentTest, HostEnvSaved, HostEnvSettings, HostEnvironment, HostId, SetupReport } from "../wire";
import { err } from "./runtime";

/**
 * Fixture-only: the hub's per-host environment settings and the host's environment stage.
 * Secret values stay in here (like the hub's secrets.json); `get` leaves them out. A setup that
 * mentions `nope` fails like a missing module; an agent command `ls`, `lister` or `python` does not answer
 * like the agent.
 */
export class FixtureHostEnvs {
  private settings = new Map<HostId, HostEnvSettings>();
  private secrets = new Map<HostId, Map<string, string>>();
  private applied = new Map<HostId, { revision: string; setup: SetupReport | null }>();
  private n = 1;

  constructor(private now: () => number) {}

  get(host: HostId): HostEnvSettings {
    return this.settings.get(host) ?? { host, setup: "", vars: [], commands: {}, revision: "" };
  }

  set(s: HostEnvSettings, up: boolean): HostEnvSaved {
    const old = this.secrets.get(s.host) ?? new Map<string, string>();
    const secrets = new Map<string, string>();
    const seen = new Set<string>();
    const vars = s.vars.map((v) => {
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(v.name)) throw err("invalid_params", `“${v.name}” is not a variable name (letters, digits and '_', not starting with a digit)`);
      if (seen.has(v.name.toUpperCase())) throw err("invalid_params", `variable ${v.name} is set twice`);
      seen.add(v.name.toUpperCase());
      if (!v.secret) return { name: v.name, value: v.value ?? "", secret: false };
      const value = v.value ?? old.get(v.name);
      if (value == null) throw err("invalid_params", `enter a value for the secret ${v.name}`);
      secrets.set(v.name, value);
      return { name: v.name, value: null, secret: true };
    });
    const commands = Object.fromEntries(Object.entries(s.commands).filter(([, c]) => c && c.trim()));
    const stored: HostEnvSettings = { host: s.host, setup: s.setup, vars, commands, revision: `rev-${this.n++}` };
    this.settings.set(s.host, stored);
    this.secrets.set(s.host, secrets);
    if (!up) return { settings: stored, not_applied: `${s.host} is not connected; the settings apply when it connects.` };
    this.apply(s.host);
    return { settings: stored, environment: null };
  }

  private apply(host: HostId) {
    const s = this.get(host);
    let setup: SetupReport | null = null;
    if (s.setup.trim()) {
      const fails = s.setup.includes("nope");
      setup = fails
        ? { ok: false, applied: true, shell: "/bin/bash -l -i", exit_code: 1, error: "the last command exited with status 1", output: "module: nope not found", duration_ms: 412 }
        : { ok: true, applied: true, shell: "/bin/bash -l -i", exit_code: 0, output: s.setup.split("\n").filter((l) => l.startsWith("module load")).map((l) => `loaded ${l.slice(12).trim()}`).join("\n"), duration_ms: 388 };
    }
    this.applied.set(host, { revision: s.revision, setup });
  }

  /** The host's report: the captured part from `base`, the rest from what was applied. */
  report(host: HostId, base: HostEnvironment): HostEnvironment {
    const a = this.applied.get(host);
    const s = this.get(host);
    if (!a) return base;
    const extra = s.setup.includes("module load") ? ["/opt/site/modules/tools/bin"] : [];
    return { ...base, revision: a.revision, setup: a.setup, variables: s.vars.map((v) => v.name), commands: s.commands, path: [...extra, ...base.path] };
  }

  /** Inventory as the host's discovery reports it with the agent commands in effect. */
  inventory(host: HostId, inv: DriverInventory[]): DriverInventory[] {
    const s = this.applied.has(host) ? this.get(host) : null;
    return inv.map((d) => {
      const c = s?.commands[d.driver];
      if (!c) return d;
      const bad = /^(ls|lister|python)\b/.test(c);
      return {
        ...d, command: c, installed: true, binary_path: `/opt/site/modules/tools/bin/${c}`, error: null,
        version: bad ? null : d.version ?? "2.1.4",
        warning: bad ? `\`${c}\` does not answer like ${d.display_name}: printed "${c} (GNU coreutils) 9.4", not a version` : null,
      };
    });
  }

  test(host: HostId, base: HostEnvironment, inv: DriverInventory[]): EnvironmentTest {
    this.apply(host);
    return { environment: this.report(host, { ...base, captured_at_ms: this.now() }), agents: this.inventory(host, inv), complete: true };
  }
}
