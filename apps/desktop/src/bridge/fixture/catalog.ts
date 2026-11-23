import type { ConfigItem, DriverCapabilities, PromptInput, DriverInventory, HostEntry, LinkState, ModelInfo, NotContinuable, ResumableSession, ResumeCommand, SessionMode, SessionSnapshot, SlashCommandInfo } from "../wire";

/** Fixture-only data: drivers and hosts as discovery and the hub would report them. */

const caps = (over: Partial<DriverCapabilities> = {}): DriverCapabilities => ({
  interrupt: true, set_model: true, set_mode: true, slash_commands: true, permission_gates: true, questions: true, plan_gate: true,
  usage: true, cost: true, resume: true, continue_latest: true, streaming_output: false, prompt_while_busy: "queued_by_cli", stop_task: false, earlier_history: "wire", ...over,
});

/** What each real driver reports (Wasit descriptors). */
const IMAGES = ["image/png", "image/jpeg", "image/gif", "image/webp"];
export const promptInput = (files: PromptInput["files"], embeds = true, maxImage = 5 * 1024 * 1024): PromptInput => ({
  image_mimes: IMAGES, max_image_bytes: maxImage, files, max_file_bytes: 50 * 1024 * 1024, max_attachments: 10, embeds_bytes: embeds,
});

export interface DriverDef {
  id: string;
  display_name: string;
  binary: string;
  capabilities: DriverCapabilities;
  prompt_input: PromptInput;
  version: string;
  models: ModelInfo[];
  modes: SessionMode[];
  slash: SlashCommandInfo[];
  config: ConfigItem[];
  /** The command with which the user opens a saved conversation in their own CLI; null: the CLI has none (the descriptor's `user_resume_argv`). */
  resumeArgv: ((nativeSessionId: string) => string[]) | null;
}

const m = (id: string, is_default = false, display_name = id, description = ""): ModelInfo => ({ id, display_name, description, is_default });
const src = (scope: string, path: string) => ({ scope, path });
const mcp = (name: string, scope: string, path: string, command: string, args: string[] = [], enabled = true, description?: string): ConfigItem => ({
  kind: "mcp_server", name, source: src(scope, path), enabled, description, mcp: { transport: "stdio", command, args, env: {} },
});

export const DRIVERS: DriverDef[] = [
  {
    id: "claude", display_name: "Claude Code", binary: "claude", capabilities: caps({ stop_task: true, earlier_history: "storage" }), prompt_input: promptInput("path_reference"), version: "2.1.4",
    models: [m("claude-sonnet-4.5", true, "claude-sonnet-4.5", "Balanced"), m("claude-opus-4.1", false, "claude-opus-4.1", "Most capable"), m("claude-haiku-4.5", false, "claude-haiku-4.5", "Fast")],
    modes: [
      { id: "default", label: "Default", description: "Ask before edits and commands" },
      { id: "acceptEdits", label: "Accept edits", description: "Edit files without asking" },
      { id: "plan", label: "Plan mode", description: "Plan first, then ask to proceed" },
      { id: "bypassPermissions", label: "Bypass permissions", description: "Never ask" },
    ],
    slash: [
      { name: "review", description: "Review uncommitted changes", argument_hint: "" },
      { name: "compact", description: "Summarize history to free context", argument_hint: "[focus]" },
      { name: "plan", description: "Switch to plan mode until you approve", argument_hint: "" },
      { name: "model", description: "Change model for this session", argument_hint: "[model]" },
      { name: "db-migrations", description: "Run the migration checklist skill", argument_hint: "" },
      { name: "cost", description: "Show token use and cost", argument_hint: "" },
    ],
    config: [
      mcp("github", "user", "~/.claude.json", "github-mcp-server"),
      mcp("postgres-ledgerline", "project", ".mcp.json", "postgres-mcp", ["postgres://localhost/ledgerline_dev"]),
      mcp("sentry", "user", "~/.claude.json", "npx", ["@sentry/mcp-server"]),
      { kind: "skill", name: "release-notes", source: src("user", "~/.claude/skills/"), enabled: true, description: "Writes CHANGELOG entries from merged PRs", path: "~/.claude/skills/release-notes" },
      { kind: "skill", name: "db-migrations", source: src("project", ".claude/skills/"), enabled: true, description: "Checklist and scripts for Prisma migrations", path: ".claude/skills/db-migrations" },
      { kind: "plugin", name: "commit-guard", source: src("user", "~/.claude/settings.json"), enabled: true, description: "Blocks commits that touch .env* or secrets/", path: "~/.claude/plugins/commit-guard" },
      { kind: "command", name: "review", source: src("project", ".claude/commands/"), enabled: true, description: "Slash command", path: ".claude/commands/review.md" },
      { kind: "command", name: "deploy-preview", source: src("project", ".claude/commands/"), enabled: true, description: "Slash command", path: ".claude/commands/deploy-preview.md" },
    ],
    resumeArgv: (id) => ["claude", "--resume", id],
  },
  {
    id: "codex", display_name: "Codex", binary: "codex", capabilities: caps({ prompt_while_busy: "steered", streaming_output: true }), prompt_input: promptInput("path_reference", false, 20 * 1024 * 1024), version: "0.58.0",
    models: [m("gpt-5-codex", true), m("gpt-5-codex-mini"), m("gpt-5"), m("o4-mini")],
    modes: [
      { id: "read-only", label: "Read only", description: "" },
      { id: "auto", label: "Auto", description: "Edit and run in the workspace" },
      { id: "full-access", label: "Full access", description: "" },
    ],
    slash: [
      { name: "review", description: "Review the working tree", argument_hint: "" },
      { name: "status", description: "Show session configuration", argument_hint: "" },
    ],
    config: [
      mcp("github", "global", "~/.codex/config.toml", "github-mcp-server"),
      mcp("postgres-ledgerline", "project", ".codex/config.toml", "postgres-mcp"),
      { kind: "skill", name: "release-notes", source: src("user", "~/.codex/skills/"), enabled: true, path: "~/.codex/skills/release-notes" },
      { kind: "command", name: "fix-ci", source: src("user", "~/.codex/prompts/"), enabled: true, description: "Custom prompt", path: "~/.codex/prompts/fix-ci.md" },
    ],
    resumeArgv: (id) => ["codex", "resume", id],
  },
  {
    id: "opencode", display_name: "OpenCode", binary: "opencode", capabilities: caps({ prompt_while_busy: "queued_by_protocol" }), prompt_input: promptInput("embedded"), version: "1.3.2",
    models: [m("anthropic/claude-sonnet-4.5", true, "claude-sonnet-4.5 (anthropic)"), m("openai/gpt-5", false, "gpt-5 (openai)")],
    modes: [
      { id: "build", label: "Build", description: "Full tool access" },
      { id: "plan", label: "Plan", description: "Read only" },
    ],
    slash: [{ name: "init", description: "Create AGENTS.md", argument_hint: "" }],
    config: [
      mcp("github", "global", "~/.config/opencode/opencode.json", "github-mcp-server"),
      { kind: "agent", name: "reviewer", source: src("global", "~/.config/opencode/agent/"), enabled: true, description: "Reviews diffs before commit", path: "~/.config/opencode/agent/reviewer.md" },
      { kind: "plugin", name: "notify", source: src("global", "~/.config/opencode/opencode.json"), enabled: true, path: "~/.config/opencode/plugin/notify" },
    ],
    resumeArgv: null,
  },
  {
    id: "kilo", display_name: "Kilo Code", binary: "kilo", capabilities: caps({ prompt_while_busy: "queued_by_protocol", plan_gate: false }), prompt_input: promptInput("embedded"), version: "7.8.27",
    models: [m("kilo/claude-sonnet-4.5", true), m("kilo/gpt-5")],
    modes: [
      { id: "code", label: "Code", description: "" },
      { id: "architect", label: "Architect", description: "" },
      { id: "debug", label: "Debug", description: "" },
    ],
    slash: [],
    config: [mcp("github", "global", "~/.config/kilo/kilo.jsonc", "github-mcp-server")],
    resumeArgv: (id) => ["kilo", "--session", id],
  },
];
export const driverDef = (id: string) => DRIVERS.find((d) => d.id === id)!;

/**
 * The conversation facts of a snapshot as the runtime derives them (`continue_verdict`, `apply_conversation`): a live session is not
 * continuable; an ended one is when its driver can resume and the agent saved the conversation (it does with the first turn).
 */
export type ConversationFacts = Pick<SessionSnapshot, "continuable" | "not_continuable" | "resume_command">;
export function conversationFacts(s: Omit<SessionSnapshot, keyof ConversationFacts>): ConversationFacts {
  const live = ["starting", "idle", "working", "needs_you"].includes(s.phase.state);
  const saved = !!s.native_session_id && (s.turns_completed > 0 || !!s.turn);
  const why: NotContinuable | null = !s.loaded ? "loading" : live ? "not_ended" : !s.capabilities.resume ? "agent_cannot_resume"
    : !s.native_session_id ? "no_conversation_id" : !saved ? "nothing_saved" : null;
  const argv = saved && s.cwd ? driverDef(s.driver)?.resumeArgv?.(s.native_session_id!) : undefined;
  const resume_command: ResumeCommand | null = argv ? { argv, cwd: s.cwd! } : null;
  return { continuable: why === null, not_continuable: why, resume_command };
}

export interface FixtureHost {
  entry: HostEntry;
  /** Initial link (`null` = the local host). */
  link: LinkState | null;
  drivers: Record<string, string>;
}

const t0 = Date.UTC(2026, 9, 1);
const linux = { os: "linux", arch: "x86_64", target: "x86_64-unknown-linux-gnu" };
export const FIXTURE_HOSTS: FixtureHost[] = [
  { entry: { id: "x1", name: "thinkpad-x1", kind: { kind: "local" }, platform: linux, created_at_ms: t0, interactive_auth: false, local_files: true }, link: null, drivers: { claude: "2.1.4", codex: "0.58.0", opencode: "1.3.2" } },
  { entry: { id: "hz", name: "hetzner-ax41", kind: { kind: "ssh", target: "dev@95.217.40.12", port: 22, identity_file: "~/.ssh/id_ed25519" }, platform: linux, created_at_ms: t0, interactive_auth: false, local_files: false },
    link: { state: "connected", hello: { protocol: { major: 1, minor: 0 }, build: "fixture", host_id: "hz", role: "daemon", features: [] } }, drivers: { claude: "2.1.4", codex: "0.58.0", opencode: "1.3.2" } },
  { entry: { id: "win", name: "win-build", kind: { kind: "ssh", target: "build@win-build.lan", port: 22 }, platform: { os: "windows", arch: "x86_64", target: "x86_64-pc-windows-msvc" }, created_at_ms: t0, interactive_auth: false, local_files: false },
    link: { state: "reconnecting", attempt: 3, next_at: { secs_since_epoch: 0, nanos_since_epoch: 0 }, last_error: "connection reset by peer" }, drivers: { claude: "2.1.2", kilo: "7.8.27" } },
  { entry: { id: "gpu", name: "gpu-box", kind: { kind: "ssh", target: "ml@10.0.4.21", port: 22 }, platform: linux, created_at_ms: t0, interactive_auth: true, local_files: false },
    link: { state: "auth_needed", problem: "auth_needed", detail: "this host signs in interactively: connect to sign in", held: true }, drivers: { claude: "2.0.9", codex: "0.51.1" } },
  { entry: { id: "pi", name: "pi-runner", kind: { kind: "ssh", target: "pi@pi-runner.lan" }, platform: { os: "linux", arch: "aarch64", target: "aarch64-unknown-linux-gnu" }, created_at_ms: t0, interactive_auth: false, local_files: false },
    link: { state: "provisioning", reason: { kind: "build_mismatch", expected: "0.7.3", found: "0.7.1" } }, drivers: { opencode: "1.3.0" } },
];

/** Sessions each CLI has on disk for a project (some started in a terminal, some by Marsad). */
export function resumable(driver: string, project: string, now: number): ResumableSession[] {
  // A folder that exists and has never had a conversation in it.
  if (/\/scratch\/?$/.test(project)) return [];
  const h = 3_600_000;
  const rows: [string, string, number][] = driver === "claude"
    ? [["Refactor invoice totals rounding", "native-7", 2 * h], ["Fix Stripe webhook idempotency", "c-term-1", 26 * h], ["", "c-term-2", 50 * h],
       ["Spike: Postgres row-level security", "native-14", 170 * h], ["Add CSV export endpoint", "c-term-3", 340 * h], ["Bump Node to 22", "c-term-4", 500 * h]]
    : [["Tidy the LRU cache tests", `${driver}-term-1`, 5 * h], ["Investigate flaky ssh tunnel test", `${driver}-term-2`, 30 * h]];
  return rows.map(([title, id, ago]) => ({ id, project, title, modified_ms: now - ago }));
}

export function inventoryFor(host: FixtureHost, project: string | null, now: number): DriverInventory[] {
  return DRIVERS.map((d) => {
    const version = host.drivers[d.id];
    return {
      driver: d.id, display_name: d.display_name, capabilities: d.capabilities, installed: !!version,
      binary_path: version ? (host.entry.platform?.os === "windows" ? `C:\\Users\\build\\AppData\\Roaming\\npm\\${d.binary}.cmd` : `/usr/local/bin/${d.binary}`) : null,
      error: version ? null : `${d.binary} was not found on PATH`,
      version: version ?? null,
      models: version ? d.models : [], modes: version ? d.modes : [],
      sessions: version && project ? resumable(d.id, project, now) : [],
      // A folder with a long history: the engine lists the newest and says how many it left out.
      ...(version && project && d.id === "claude" && /\/archive\/?$/.test(project) ? { not_listed: { sessions: 340, config: 0, models: 0, modes: 0 } } : {}),
      config: version ? d.config : [], project, refreshing: false,
      probes: version ? [{ kind: "version", ok: true, duration_ms: 41 }, { kind: "models", ok: true, duration_ms: 230 }, { kind: "sessions", ok: true, duration_ms: 88 }] : [{ kind: "version", ok: false, error: "not installed", duration_ms: 3 }],
      refreshed_at_ms: now - 4 * 60_000,
    };
  });
}
