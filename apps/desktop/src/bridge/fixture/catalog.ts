import type { DriverCapabilities, DriverSummary, HostId, HostInfo, HostLink, ModelInfo, SessionMode, SlashCommandInfo } from "../types";

/** Fixture-only data: drivers, models, modes and hosts as the runtime would report them. */

const caps = (over: Partial<DriverCapabilities> = {}): DriverCapabilities => ({
  interrupt: true,
  set_model: true,
  set_mode: true,
  slash_commands: true,
  permission_gates: true,
  questions: true,
  plan_gate: true,
  usage: true,
  cost: true,
  resume: true,
  continue_latest: true,
  streaming_output: false,
  prompt_while_busy: "queued_by_cli",
  ...over,
});

export const DRIVER_CATALOG: (DriverSummary & { models: ModelInfo[]; modes: SessionMode[]; slash: SlashCommandInfo[]; version: string })[] = [
  {
    id: "claude",
    display_name: "Claude Code",
    capabilities: caps(),
    version: "2.1.4",
    models: [
      { id: "claude-sonnet-4.5", display_name: "claude-sonnet-4.5", description: "Balanced", is_default: true },
      { id: "claude-opus-4.1", display_name: "claude-opus-4.1", description: "Most capable", is_default: false },
      { id: "claude-haiku-4.5", display_name: "claude-haiku-4.5", description: "Fast", is_default: false },
    ],
    modes: [
      { id: "default", label: "Default mode", description: "Ask before edits and commands" },
      { id: "acceptEdits", label: "Accept edits", description: "Edit files without asking" },
      { id: "plan", label: "Plan mode", description: "Plan first, then ask to proceed" },
    ],
    slash: [
      { name: "review", description: "Review uncommitted changes", argument_hint: "" },
      { name: "compact", description: "Summarize history to free context", argument_hint: "[focus]" },
      { name: "plan", description: "Switch to plan mode until you approve", argument_hint: "" },
      { name: "model", description: "Change model for this session", argument_hint: "[model]" },
      { name: "db-migrations", description: "Run the migration checklist skill", argument_hint: "" },
      { name: "cost", description: "Show token use and cost", argument_hint: "" },
    ],
  },
  {
    id: "codex",
    display_name: "Codex",
    capabilities: caps({ prompt_while_busy: "steered", streaming_output: true }),
    version: "0.58.0",
    models: [
      { id: "gpt-5-codex", display_name: "gpt-5-codex", description: "", is_default: true },
      { id: "gpt-5-codex-mini", display_name: "gpt-5-codex-mini", description: "", is_default: false },
    ],
    modes: [
      { id: "read-only", label: "Read only", description: "" },
      { id: "auto", label: "Auto", description: "" },
      { id: "full-access", label: "Full access", description: "" },
    ],
    slash: [
      { name: "review", description: "Review the working tree", argument_hint: "" },
      { name: "status", description: "Show session configuration", argument_hint: "" },
    ],
  },
  {
    id: "opencode",
    display_name: "OpenCode",
    capabilities: caps({ prompt_while_busy: "queued_by_protocol" }),
    version: "0.15.2",
    models: [{ id: "anthropic/claude-sonnet-4.5", display_name: "claude-sonnet-4.5 (anthropic)", description: "", is_default: true }],
    modes: [
      { id: "build", label: "Build", description: "Full tool access" },
      { id: "plan", label: "Plan", description: "Read only" },
    ],
    slash: [{ name: "init", description: "Create AGENTS.md", argument_hint: "" }],
  },
  {
    id: "kilo",
    display_name: "Kilo Code",
    capabilities: caps({ prompt_while_busy: "queued_by_protocol", plan_gate: false }),
    version: "4.2.0",
    models: [{ id: "kilo/auto", display_name: "auto", description: "", is_default: true }],
    modes: [
      { id: "code", label: "Code", description: "" },
      { id: "architect", label: "Architect", description: "" },
    ],
    slash: [],
  },
];

export interface FixtureHost {
  id: HostId;
  name: string;
  where: "local" | "ssh";
  os: string;
  drivers: string[];
  link: HostLink["link"];
  retry_in_ms?: number;
}

export const FIXTURE_HOSTS: FixtureHost[] = [
  { id: "x1", name: "thinkpad-x1", where: "local", os: "Ubuntu 24.04", drivers: ["claude", "codex", "opencode"], link: "connected" },
  { id: "hz", name: "hetzner-ax41", where: "ssh", os: "Debian 12", drivers: ["claude", "codex", "opencode"], link: "connected" },
  { id: "win", name: "win-build", where: "ssh", os: "Windows 11 Pro", drivers: ["claude", "kilo"], link: "reconnecting", retry_in_ms: 4000 },
];

export function hostInfo(h: FixtureHost, now: number): HostInfo {
  return {
    host_id: h.id,
    build: "fixture",
    pid: 4242,
    started_at_ms: now - 86_400_000,
    data_dir: "~/.local/share/wasit",
    os: h.os,
    arch: "x86_64",
    drivers: DRIVER_CATALOG.filter((d) => h.drivers.includes(d.id)).map(({ id, display_name, capabilities }) => ({ id, display_name, capabilities })),
  };
}
