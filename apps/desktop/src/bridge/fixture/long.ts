import { DRIVERS, FIXTURE_HOSTS } from "./catalog";
import { SPECS } from "./sessions";

/**
 * `?long=1`: realistic long strings everywhere a side panel shows a value (absolute skill paths
 * on Linux and Windows, long host names and addresses, version strings, error messages,
 * branches), to catch layouts that overflow sideways. Mutates the fixture tables in place, so
 * it runs once, before the backend is built.
 */
export function applyLongStrings() {
  const claude = DRIVERS.find((d) => d.id === "claude")!;
  const home = "/home/someone-with-a-long-username";
  claude.config = [
    ...claude.config.filter((i) => i.kind !== "skill"),
    ...["some-long-skill-name", "another-rather-long-skill-for-database-migrations", "release-notes-from-merged-pull-requests"].map((name) => ({
      kind: "skill" as const,
      name,
      source: { scope: "user", path: `${home}/.claude/skills/${name}/SKILL.md` },
      enabled: true,
      description: `Skill read from ${home}/.claude/skills/${name}/SKILL.md`,
      path: `${home}/.claude/skills/${name}`,
    })),
    {
      kind: "command",
      name: "deploy-preview-environment-for-the-current-branch",
      source: { scope: "project", path: "C:\\Users\\someone-with-a-long-username\\src\\ledgerline\\.claude\\commands\\deploy-preview-environment-for-the-current-branch.md" },
      enabled: true,
      description: "Slash command",
      path: "C:\\Users\\someone-with-a-long-username\\src\\ledgerline\\.claude\\commands\\deploy-preview-environment-for-the-current-branch.md",
    },
  ];
  for (const d of DRIVERS) d.version = `${d.version} (${d.display_name}) build 2026-11-03T12:44:10Z+sha.0123456789abcdef0123`;

  const hz = FIXTURE_HOSTS.find((h) => h.entry.id === "hz")!;
  hz.entry.name = "hetzner-ax41-build-runner-in-falkenstein-datacenter-fsn1-dc14";
  hz.entry.kind = { kind: "ssh", target: "deployment-automation@hetzner-ax41-build-runner.internal.example-company.com", port: 22, identity_file: "C:\\Users\\someone-with-a-long-username\\.ssh\\id_ed25519_hetzner_build_runner" };
  for (const k of Object.keys(hz.drivers)) hz.drivers[k] = `${hz.drivers[k]}-nightly.20261103.0123456789abcdef`;
  const win = FIXTURE_HOSTS.find((h) => h.entry.id === "win")!;
  win.link = {
    state: "reconnecting", attempt: 3, next_at: { secs_since_epoch: 0, nanos_since_epoch: 0 },
    last_error: "no marsad-agent build for x86_64-unknown-linux-gnu at C:\\Users\\someone-with-a-long-username\\AppData\\Local\\Marsad\\agents\\x86_64-unknown-linux-gnu\\marsad-agent (ssh: connect to host win-build.lan port 22: Connection timed out after 15000ms)",
  };

  for (const s of SPECS) {
    if (s.host === "hz") s.branch = `feature/${s.branch.replace("/", "-")}-with-a-very-long-descriptive-branch-name-for-review`;
  }
}
