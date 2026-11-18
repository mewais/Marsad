#!/usr/bin/env node
// End-to-end suite against the REAL desktop app on the REAL backend: the Tauri binary (with the
// built UI embedded), its local hub daemon (`marsad-agent`), session shims and agent CLIs, driven
// through the real DOM over WebDriver (tauri-driver + WebKitWebDriver).
//
//   node e2e-real/run.mjs [--build] [--real] [--shots <dir>] [--only <name,...>] [--keep]
//
// --build   build marsad-agent, the fake CLI, the UI and the app first
// --real    also run the live scenarios with the agent CLIs installed on this machine (claude,
//           opencode, kilo; codex stays the fake persona). They cost a few cents and need logins.
// --shots   save named screenshots (1440x900) of every screen into <dir>
// --only    run only the named scenarios
// --keep    keep the temporary data directory
//
// Needs: tauri-driver (`cargo install tauri-driver`), WebKitWebDriver (`webkit2gtk-driver`), git,
// and a display (run under `xvfb-run -a dbus-run-session`; `pnpm test:real` does that).

import { spawn, spawnSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { WebDriver, Keys } from "./wd.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const desktop = path.resolve(here, "..");
const repo = path.resolve(desktop, "../..");
const args = process.argv.slice(2);
const flag = (n) => args.includes(n);
const opt = (n) => (args.includes(n) ? args[args.indexOf(n) + 1] : undefined);
const REAL = flag("--real");
const SHOTS = opt("--shots");
const ONLY = opt("--only")?.split(",");
const KEEP = flag("--keep");

const bin = {
  agent: path.join(repo, "target/debug/marsad-agent"),
  fake: path.join(repo, "target/debug/wasit-fake-cli"),
  app: path.join(desktop, "src-tauri/target/debug/marsad-desktop"),
};

function sh(cmd, argv, opts = {}) {
  const r = spawnSync(cmd, argv, { stdio: "inherit", ...opts });
  if (r.status !== 0) throw new Error(`${cmd} ${argv.join(" ")} failed (${r.status})`);
}

if (flag("--build")) {
  sh("cargo", ["build", "-p", "marsad-agent", "-p", "marsad-fake-cli"], { cwd: repo });
  sh("pnpm", ["build"], { cwd: desktop });
  sh("cargo", ["build", "--features", "custom-protocol"], { cwd: path.join(desktop, "src-tauri") });
}
for (const [k, p] of Object.entries(bin)) if (!fs.existsSync(p)) throw new Error(`${k} binary missing: ${p} (run with --build)`);

// ── fixtures on disk ────────────────────────────────────────────────────────

const root = fs.mkdtempSync(path.join(os.tmpdir(), "marsad-e2e-"));
const dirs = { data: path.join(root, "data"), cfg: path.join(root, "cfg"), work: path.join(root, "work"), shots: SHOTS && path.resolve(SHOTS) };
for (const d of Object.values(dirs)) if (d) fs.mkdirSync(d, { recursive: true });
const git = (cwd, ...a) => execFileSync("git", a, { cwd, stdio: "pipe" }).toString();

function makeRepo(name, files) {
  const d = path.join(dirs.work, name);
  fs.mkdirSync(d, { recursive: true });
  git(d, "init", "-q", "-b", "main");
  git(d, "config", "user.email", "e2e@example.com");
  git(d, "config", "user.name", "E2E");
  for (const [f, c] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(d, f)), { recursive: true });
    fs.writeFileSync(path.join(d, f), c);
  }
  git(d, "add", ".");
  git(d, "commit", "-q", "-m", "initial import");
  return d;
}

const project = makeRepo("ledgerline", {
  "README.md": "# ledgerline\n\nInvoices and payments.\n",
  "src/server.ts": "export function start(port: number) {\n  return port;\n}\n",
  "src/auth.ts": "export const SESSION_COOKIE = 'sid';\n",
});

// One scenario for every fake persona; turns are picked by a regex on the prompt.
const scenario = {
  models: [
    { id: "fake/sonnet-4", name: "Sonnet 4 (fake)", description: "balanced" },
    { id: "fake/haiku-4", name: "Haiku 4 (fake)", description: "fast" },
  ],
  current_model: "fake/sonnet-4",
  modes: [
    { id: "build", name: "Build", description: "Edits and runs tools" },
    { id: "plan", name: "Plan", description: "Read-only planning" },
  ],
  current_mode: "build",
  commands: [{ name: "review", description: "Review the working tree" }],
  sessions: [{ id: "fake-old-1", title: "Earlier: tidy the README", cwd: project, history: [{ role: "user", text: "tidy the README" }, { role: "assistant", text: "Done." }] }],
  echo_prefix: "echo: ",
  turns: [
    { when: "run the tests", steps: [
      { type: "say", text: "I'll run the test suite first." },
      { type: "tool", name: "Bash", input: { command: "pnpm test --filter auth" }, needs_permission: true, result_if_allowed: "12 passed", result_if_denied: "not run" },
      { type: "usage", input_tokens: 5200, output_tokens: 410, cached_tokens: 3100, cost_usd: 0.031, context_window: 200000 },
      { type: "say", text: "All 12 auth tests pass." },
    ] },
    { when: "which approach", steps: [
      { type: "ask_question", question: "Which cookie migration should I implement?", header: "Rollout", options: [
        { label: "Migrate on first request", description: "Reads the old cookie and issues a new token" },
        { label: "Invalidate at deploy", description: "Everyone signs in again once" },
      ], multi_select: false },
      { type: "say", text: "Going with that." },
    ] },
    { when: "make a plan", steps: [
      { type: "plan", entries: [
        { content: "Add the token store", status: "pending", priority: "high" },
        { content: "Migrate the session middleware", status: "pending", priority: "high" },
        { content: "Delete the legacy cookie path", status: "pending", priority: "medium" },
      ], needs_approval: true },
      { type: "say", text: "Plan approved; starting with the token store." },
    ] },
    { when: "deploy", steps: [
      { type: "say", text: "Deploying the billing service to staging." },
      { type: "tool", name: "Bash", input: { command: "pnpm deploy --env staging" }, needs_permission: false, result_if_allowed: "error: STAGING_DB_URL is not set" },
      { type: "say", text: "The deploy failed: STAGING_DB_URL is missing from the staging environment." },
      { type: "exit", code: 1 },
    ] },
    { when: "migrate the session", steps: [
      { type: "think", text: "The middleware reads the legacy cookie in two places." },
      { type: "tool", name: "Read", input: { file_path: "src/auth.ts" }, needs_permission: false, result_if_allowed: "export const SESSION_COOKIE = 'sid';" },
      { type: "tool", name: "Edit", input: { file_path: "src/auth.ts" }, needs_permission: false, result_if_allowed: "1 replacement" },
      { type: "usage", input_tokens: 18400, output_tokens: 1260, cached_tokens: 12100, cost_usd: 0.094, context_window: 200000 },
      { type: "say", text: "Migrated: the middleware now issues a signed token and accepts the legacy cookie for 30 days." },
    ] },
    { when: "render the", steps: [
      { type: "say", text: "Rendering the export now; this takes a few minutes." },
      { type: "tool", name: "Bash", input: { command: "node scripts/render-invoice.mjs INV-2041" }, needs_permission: false, result_if_allowed: "page 1/4" },
      { type: "sleep_ms", ms: 1800000 },
      { type: "say", text: "Rendered." },
    ] },
    { when: "long task", steps: [
      { type: "say", text: "Starting the long refactor." },
      { type: "plan", entries: [{ content: "Refactor the session store", status: "in_progress", priority: "high" }], needs_approval: false },
      { type: "wait_for_interrupt" },
    ] },
  ],
};
const scenarioPath = path.join(root, "scenario.json");
fs.writeFileSync(scenarioPath, JSON.stringify(scenario, null, 2));

const FAKE = REAL ? ["codex"] : ["claude", "codex", "opencode", "kilo"];
const daemonArgs = ["--stop-grace-ms", "1500"];
for (const d of FAKE) daemonArgs.push("--driver-bin", `${d}=${bin.fake}`, "--driver-env", `${d}=WASIT_FAKE_PERSONA=${d}`, "--driver-env", `${d}=WASIT_FAKE_SCENARIO=${scenarioPath}`);
// The app's own XDG dirs are this run's temp dirs (window state etc.); the real CLIs keep the
// user's: their logins, providers and settings live there.
const home = os.homedir();
const userXdg = { XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME ?? path.join(home, ".config"), XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(home, ".local/share") };
if (REAL) for (const d of ["claude", "opencode", "kilo"]) for (const [k, v] of Object.entries(userXdg)) daemonArgs.push("--driver-env", `${d}=${k}=${v}`);

// ── the app under WebDriver ──────────────────────────────────────────────────

const freePort = () => new Promise((res) => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => res(p)); }); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const port = await freePort();
const nativePort = await freePort();
const env = {
  ...process.env,
  WEBKIT_DISABLE_COMPOSITING_MODE: "1",
  MARSAD_DATA: dirs.data,
  MARSAD_AGENT_BIN: bin.agent,
  MARSAD_DAEMON_ARGS: daemonArgs.join(" "),
  MARSAD_LOG: process.env.MARSAD_LOG ?? "info",
  XDG_CONFIG_HOME: dirs.cfg,
  XDG_DATA_HOME: path.join(dirs.cfg, "share"),
  // The shell's notification decisions (test hook), one JSON line each.
  MARSAD_NOTIFY_LOG: path.join(root, "notifications.ndjson"),
};
const driverLog = fs.openSync(path.join(root, "tauri-driver.log"), "a");
const driver = spawn("tauri-driver", ["--port", String(port), "--native-port", String(nativePort)], { env, stdio: ["ignore", driverLog, driverLog] });
for (let i = 0; i < 100; i++) {
  if (await fetch(`http://127.0.0.1:${port}/status`).then(() => true, () => false)) break;
  await sleep(100);
}

const wd = new WebDriver(`http://127.0.0.1:${port}`);
const results = [];
let failed = 0;

async function startApp() {
  await wd.start({ browserName: "wry", "tauri:options": { application: bin.app } });
  await wd.setRect(1440, 900);
  await waitFor(() => wd.exec(() => !!document.querySelector(".ibx-item, .placeholder, .empty") && !document.body.innerText.includes("Connecting to your hosts")), "fleet ready", 60_000);
}

// ── helpers (polling here is the harness waiting on the UI, not product code) ──

async function waitFor(fn, what, ms = 30_000) {
  const end = Date.now() + ms;
  let last;
  while (Date.now() < end) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    await sleep(150);
  }
  throw new Error(`timed out waiting for ${what}${last ? ` (${last.message})` : ""}`);
}

const xq = (s) => (s.includes("'") ? `concat('${s.split("'").join(`', "'", '`)}')` : `'${s}'`);
/** The first visible element matching CSS whose text contains `text` (or any when omitted). */
async function el(css, text, ms = 20_000) {
  return waitFor(async () => {
    const found = await wd.exec((css, text) => {
      const all = [...document.querySelectorAll(css)].filter((e) => e.getClientRects().length && (!text || e.textContent.replace(/\s+/g, " ").includes(text)));
      const e = all[0];
      if (!e) return null;
      e.setAttribute("data-e2e-hit", String(Date.now()));
      return e.getAttribute("data-e2e-hit");
    }, css, text ?? "");
    return found ? wd.find("css selector", `[data-e2e-hit="${found}"]`) : null;
  }, `${css}${text ? ` with "${text}"` : ""}`, ms);
}
/** Click like a user; a toast lying over the target is dismissed first (as a user would). */
async function click(css, text, ms) {
  const e = await el(css, text, ms);
  try {
    await wd.click(e);
  } catch (err) {
    if (/stale/.test(err.message)) return wd.click(await el(css, text, ms));
    if (!/intercepted/.test(err.message)) throw err;
    await wd.exec(() => document.querySelectorAll(".toast button[aria-label=Dismiss]").forEach((b) => b.click()));
    await sleep(300);
    await wd.click(await el(css, text, ms));
  }
}
const button = (text, ms) => click("button, [role=radio], [role=option], [role=tab], a", text, ms);
async function type(css, text, { clear = true } = {}) {
  for (let i = 0; ; i++) {
    try {
      const e = await el(css);
      if (clear) await wd.clear(e);
      await wd.sendKeys(e, text);
      return;
    } catch (err) {
      // React replaced the node between lookup and typing: look it up again.
      if (i >= 3 || !/stale/.test(err.message)) throw err;
      await sleep(200);
    }
  }
}
const bodyHas = (text, ms = 30_000) => waitFor(() => wd.exec((t) => document.body.innerText.includes(t), text), `text "${text}"`, ms);
const bodyLacks = (text, ms = 30_000) => waitFor(() => wd.exec((t) => !document.body.innerText.includes(t), text), `no text "${text}"`, ms);
const go = (hash) => wd.exec((h) => { location.hash = h; }, hash);
/** Call the hub through the same bridge the UI uses (setup and checks only). */
const rpc = (method, params = null, host = null) => wd.execAsync((m, p, h) => window.__TAURI__.core.invoke("rpc", { method: m, params: p, host: h }), method, params, host);
const state = () => wd.exec(() => window.__marsad?.state?.() ?? null);

async function shot(name) {
  if (!dirs.shots) return;
  await sleep(400);
  fs.writeFileSync(path.join(dirs.shots, `${name}.png`), await wd.screenshot());
}

async function fleetSessions() {
  const hosts = await rpc("hosts.list");
  void hosts;
  return rpc("sessions.list", null, null).then((r) => r.sessions);
}

async function sessionByTitle(title, ms = 40_000) {
  return waitFor(async () => (await fleetSessions()).find((s) => (s.display_name ?? s.title ?? "").includes(title)), `session "${title}"`, ms);
}

const openSession = async (s, tab = "") => {
  await go(`#/session/${encodeURIComponent(local)}/${s.id}${tab ? `?tab=${tab}` : ""}`);
  await el(".sd-head");
};

/** Launch through the New session dialog (real DOM), return the snapshot once it exists. */
async function launchViaDialog({ agent, cwd, prompt, latest = false, resumeTitle, resumeId, model }) {
  await go("#/inbox");
  await button("New session");
  await el("[role=dialog]");
  // The host's discovery may still be probing the CLIs: wait until this one is offered.
  await click("[role=dialog] [role=radio]:not([disabled])", agent, 120_000);
  // The dialog pre-fills the most recent folder when its list arrives; type until it holds ours.
  await waitFor(async () => {
    if (await wd.exec(() => document.querySelector("#ns-cwd")?.value) === cwd) return true;
    await wd.exec(() => { const i = document.querySelector("#ns-cwd"); i.select(); });
    await wd.sendKeys(await el("#ns-cwd"), Keys.BACKSPACE + cwd);
    await sleep(300);
    return (await wd.exec(() => document.querySelector("#ns-cwd")?.value)) === cwd;
  }, "the project folder typed");
  if (model) {
    await click("[role=dialog] button[aria-label^='Model:']", null, 90_000);
    const filter = await wd.findAll("css selector", "input[aria-label='Filter Model']");
    if (filter.length) await wd.sendKeys(filter[0], model);
    await click("[role=listbox][aria-label=Model] [role=option]", model, 30_000);
  }
  if (latest || resumeTitle || resumeId) {
    await button("Continue an existing one");
    if (latest) {
      // "Continue most recent" (the CLI's own continue) is the default when the CLI has it.
      const b = await el("[role=dialog] .recent button");
      if ((await wd.text(b)).trim() !== "Selected") await wd.click(b);
    }
    else if (resumeId) await click(`[role=option][data-id="${resumeId}"]`, null, 60_000);
    else await click("[role=option]", resumeTitle, 60_000);
  }
  if (prompt) await type("#ns-prompt", prompt);
  const before = new Set((await fleetSessions()).map((s) => s.id));
  await click("[role=dialog] button.primary:not([disabled])", null, 60_000);
  return waitFor(async () => (await fleetSessions()).find((s) => !before.has(s.id)), "the launched session", 60_000);
}

const snapOf = async (id) => (await fleetSessions()).find((s) => s.id === id);
const waitPhase = (id, phases, ms = 60_000) => waitFor(async () => { const s = await snapOf(id); return s && [].concat(phases).includes(s.phase.state) && s; }, `session ${id} ${phases}`, ms);
async function sendPrompt(text) {
  await type("textarea[aria-label=Message]", text);
  await button("Send");
}

// ── scenarios ───────────────────────────────────────────────────────────────

let local = "";
const scenarios = [];
const scenario_ = (name, fn, { real = false } = {}) => scenarios.push({ name, fn, real });

const DRIVER_NAMES = { claude: "Claude Code", codex: "Codex", opencode: "OpenCode", kilo: "Kilo Code" };
const launched = {};

scenario_("inbox-ready", async () => {
  await go("#/inbox");
  await bodyHas("Inbox");
  const info = await rpc("daemon.info");
  if (!info.socket) throw new Error("daemon.info has no socket");
  await shot("00-inbox-empty-dark");
});

for (const d of ["claude", "codex", "opencode", "kilo"]) {
  scenario_(`launch-${d}`, async () => {
    if (REAL && d !== "codex") return "skipped in --real (live scenarios cover it)";
    const s = await launchViaDialog({ agent: DRIVER_NAMES[d], cwd: project, prompt: `hello from ${d}` });
    launched[d] = s;
    await waitPhase(s.id, ["idle"]);
    await openSession(s);
    await bodyHas(`echo: hello from ${d}`);
  });

  scenario_(`permission-gate-${d}`, async () => {
    const s = launched[d];
    if (!s) return "no session";
    await openSession(s);
    await sendPrompt("please run the tests");
    await waitPhase(s.id, "needs_you");
    await el("[data-gate=permission]");
    // The choices are the driver's own: ACP option names; neutral labels otherwise.
    const labels = await wd.exec(() => [...document.querySelectorAll("[data-gate=permission] .choices button")].map((b) => b.getAttribute("data-choice")));
    const expect = { claude: ["allow", "deny"], codex: ["accept"], opencode: [], kilo: [] }[d];
    for (const e of expect) if (!labels.includes(e)) throw new Error(`${d}: choice ${e} missing in ${labels}`);
    if (d === "claude") await shot("10-session-permission-gate-claude");
    await click("[data-gate=permission] .choices button", "Allow");
    await waitPhase(s.id, "idle");
    await bodyHas("All 12 auth tests pass.");
  });

  scenario_(`question-gate-${d}`, async () => {
    const s = launched[d];
    if (!s) return "no session";
    await openSession(s);
    await sendPrompt("which approach do you suggest?");
    await waitPhase(s.id, "needs_you");
    if (d === "opencode" || d === "kilo") {
      // ACP has no question request: the fake (like the real CLIs) asks with permission options.
      await click("[data-gate] .choices button", "Migrate on first request");
    } else {
      await el("[data-gate=question]");
      if (d === "claude") await shot("11-session-question-gate-claude");
      await click("[data-gate=question] [role=radio]", "Migrate on first request");
      await button("Send answer");
    }
    await waitPhase(s.id, "idle");
    await bodyHas("Going with that.");
  });
}

scenario_("plan-gate-claude", async () => {
  const s = launched.claude;
  if (!s) return "no session";
  await openSession(s);
  await sendPrompt("make a plan for the migration");
  await waitPhase(s.id, "needs_you");
  await el("[data-gate=permission]", "Plan");
  await click("[data-gate=permission] .choices button", "Approve plan");
  await waitPhase(s.id, "idle");
  await bodyHas("Plan approved");
});

scenario_("interrupt", async () => {
  const s = launched.opencode ?? launched.codex;
  await openSession(s);
  await sendPrompt("start the long task");
  await waitPhase(s.id, "working");
  await bodyHas("Starting the long refactor.");
  await shot("12-session-working");
  await button("Interrupt");
  await waitPhase(s.id, "idle");
});

scenario_("model-mode-slash", async () => {
  const s = launched.opencode ?? launched.codex;
  await openSession(s, "config");
  await click("button[aria-label^='Model:']");
  await click("[role=option]", "Haiku 4 (fake)");
  await click("button[aria-label^='Mode:']");
  await click("[role=option]", "Plan");
  await button("Apply to running session");
  await waitFor(async () => { const x = await snapOf(s.id); return x.model === "fake/haiku-4" && x.mode === "plan"; }, "model and mode applied");
  await shot("13-session-config");
  await openSession(s);
  await type("textarea[aria-label=Message]", "/rev");
  await el("[role=listbox][aria-label='Slash commands']");
  await wd.sendKeys(await el("textarea[aria-label=Message]"), Keys.ENTER);
  await waitPhase(s.id, "idle");
});

scenario_("rename", async () => {
  const s = launched.claude ?? launched.codex;
  await openSession(s);
  await click("button[aria-label='Rename session']");
  const input = await el("input[aria-label='Session name']");
  await wd.clear(input);
  await wd.sendKeys(input, "Auth cookie migration" + Keys.ENTER);
  await waitFor(async () => (await snapOf(s.id)).display_name === "Auth cookie migration", "display name stored");
  await go("#/inbox");
  await bodyHas("Auth cookie migration");
});

scenario_("changes-tab", async () => {
  const s = launched.claude ?? launched.codex;
  fs.appendFileSync(path.join(project, "src/server.ts"), "export const VERSION = 2;\n");
  fs.writeFileSync(path.join(project, "src/tokens.ts"), "export const TOKEN_TTL = 3600;\n");
  fs.writeFileSync(path.join(project, "scratch.txt"), "throwaway\n");
  await rpc("fleet.git_refresh", { host: local, cwd: project });
  // The hub's background git summary (what list rows show) catches up without a request per row.
  await waitFor(async () => (await state())?.git?.[local]?.[project]?.files === 3, "git summary with 3 files");
  await openSession(s, "changes");
  await el("[role=option]", "src/server.ts");
  await click("[role=option]", "src/server.ts");
  await bodyHas("VERSION = 2");
  await shot("14-session-changes");
  // Discard the scratch file, commit the rest.
  await click("input[aria-label='Include src/server.ts']");
  await click("input[aria-label='Include src/tokens.ts']");
  await button("Discard");
  await click("[role=dialog] button", "Discard");
  await waitFor(() => !fs.existsSync(path.join(project, "scratch.txt")), "scratch discarded");
  await click("[role=option]", "src/server.ts");
  await type("#commit-msg", "Add VERSION and token TTL");
  await button("Commit");
  await waitFor(() => git(project, "log", "-1", "--format=%s").trim() === "Add VERSION and token TTL", "commit created");
});

scenario_("terminal-tab", async () => {
  const s = launched.claude ?? launched.codex;
  await openSession(s, "terminal");
  await el(".xterm-box .xterm");
  await click(".xterm-box .xterm");
  await wd.sendKeys(await el(".xterm-box textarea"), "echo marsad-term-$((40+2))" + Keys.ENTER);
  await waitFor(() => wd.exec(() => document.querySelector(".xterm-box .xterm-rows")?.textContent.includes("marsad-term-42")), "terminal output");
  await shot("15-session-terminal");
});

scenario_("continue-latest-and-resume", async () => {
  // The fake opencode lists its sessions like the real one (`session list`); --real covers
  // claude with a conversation started in a terminal.
  if (REAL) return "covered by live-*-continue-and-resume";
  const d = "opencode";
  const s = await launchViaDialog({ agent: DRIVER_NAMES[d], cwd: project, prompt: "pick up the README work", resumeTitle: "Earlier: tidy the README" });
  await waitPhase(s.id, ["idle", "working"]);
  if (!(await snapOf(s.id)).native_session_id) throw new Error("resumed session has no native id");
  const l = await launchViaDialog({ agent: DRIVER_NAMES[d], cwd: project, prompt: "continue please", latest: true });
  await waitPhase(l.id, ["idle", "working"]);
});

scenario_("stop-delete", async () => {
  const s = launched.kilo ?? launched.codex;
  await openSession(s);
  await button("Stop");
  await waitPhase(s.id, "exited");
  await button("Delete");
  await click("[role=dialog] button", "Delete session");
  await waitFor(async () => !(await snapOf(s.id)), "session deleted");
  await bodyHas("Inbox");
});

scenario_("agent-config", async () => {
  await go("#/config");
  await bodyHas("Agent config");
  await shot("20-agent-config");
});

scenario_("workstream", async () => {
  // Design mode: create with three nodes over the API's draft, then everything in the UI.
  const node = (id, name, role, prompt) => ({ id, name, role, description: "", session: null, launch: { host: null, driver: REAL ? "codex" : "claude", cwd: project, model: null, mode: null, first_prompt: prompt, labels: {}, overrides: { items: [], env: [], disallowed_tools: [] } } });
  const ws = await rpc("workstreams.create", {
    name: "Invoice export v2",
    description: "Orchestrator with two workers",
    nodes: [node("orch", "orchestrator", "lead", "coordinate the invoice export"), node("pdf", "pdf-worker", "pdf", null), node("csv", "csv-worker", "csv", null)],
    edges: [{ from: "orch", to: "pdf", policy: "ask" }, { from: "orch", to: "csv", policy: "ask" }],
    limits: {},
    layout: {},
  });
  await go(`#/workstreams/${ws.id}`);
  await bodyHas("Invoice export v2");
  await shot("30-workstream-design");
  await button("Launch");
  const v = await waitFor(async () => (await rpc("workstreams.get", { id: ws.id })).nodes.every((n) => n.session) && rpc("workstreams.get", { id: ws.id }), "nodes launched", 60_000);
  const orch = v.nodes.find((n) => n.id === "orch").session;
  // Wait until the topology is in force, then the orchestrator delegates (the call its MCP tool makes).
  await sleep(1500);
  const delegate = (to) => rpc("wasl.delegate", { from: orch.session, to: [to], mode: "command", instruction: `render the ${to} export for INV-2041` }, orch.host);
  await delegate("pdf-worker");
  await el("[data-gate=handoff]", null, 30_000);
  await shot("31-workstream-handoff-gate");
  await click("[data-gate=handoff] .choices button", "Allow once");
  await sleep(500);
  await delegate("csv-worker");
  await el("[data-gate=handoff]", null, 30_000);
  await click("[data-gate=handoff] .choices button", "Always allow");
  await waitFor(async () => (await rpc("workstreams.get", { id: ws.id })).edges.find((e) => e.to === "csv")?.policy === "always_allow", "edge policy written");
  await go(`#/workstreams/${ws.id}`);
  await shot("32-workstream-graph-running");
});

scenario_("daemon-kill-recovery", async () => {
  const s = launched.codex;
  if (!s) return "no session";
  await openSession(s);
  await sendPrompt("please run the tests");
  await waitPhase(s.id, "needs_you");
  const info = await rpc("daemon.info");
  process.kill(info.pid, "SIGKILL");
  // The shell restarts the daemon on demand; the session (held by its shim) still waits.
  await waitFor(async () => (await rpc("daemon.info").catch(() => ({}))).pid && (await rpc("daemon.info")).pid !== info.pid, "daemon restarted", 60_000);
  await waitPhase(s.id, "needs_you");
  await click("[data-gate=permission] .choices button", "Allow", 30_000);
  await waitPhase(s.id, "idle");
});

scenario_("notifications-follow-prefs", async () => {
  // Set through the Settings page, checked through the shell's notification log.
  const s = launched.codex;
  if (!s) return "no session";
  const log = env.MARSAD_NOTIFY_LOG;
  const entries = () => (fs.existsSync(log) ? fs.readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l)) : []);
  const setPrefs = async (want) => {
    await go("#/settings");
    await button("Notifications");
    for (const [label, on] of Object.entries(want)) {
      const sw = await el(`button[role=switch][aria-label^='${label}']`);
      if ((await wd.attr(sw, "aria-checked")) !== String(on)) await wd.click(sw);
      await waitFor(async () => (await wd.attr(await el(`button[role=switch][aria-label^='${label}']`), "aria-checked")) === String(on), `${label} ${on}`);
    }
    await shot("60-settings-notifications");
  };
  // The app is focused under the test display: let notifications through while focused.
  await rpc("store.settings.set", { key: "notify.only_unfocused", value: false });
  const round = async (prefs, expect) => {
    await setPrefs({ "Needs you": prefs.needs, "A session finished": prefs.finished });
    const n0 = entries().length;
    await openSession(s);
    await sendPrompt("please run the tests");
    await waitPhase(s.id, "needs_you");
    await waitFor(() => entries().slice(n0).some((e) => e.kind === "NeedsYou"), "a needs-you decision", 20_000);
    await click("[data-gate=permission] .choices button", "Allow");
    await waitPhase(s.id, "idle");
    await go("#/inbox"); // looking away: the finished turn is unseen
    await waitFor(() => entries().slice(n0).some((e) => e.kind === "Finished"), "a finished decision", 20_000).catch(() => {});
    const got = entries().slice(n0);
    for (const [kind, shown] of Object.entries(expect)) {
      const e = got.find((x) => x.kind === kind);
      if (!e) throw new Error(`no ${kind} decision (${JSON.stringify(got)})`);
      if ((e.suppressed === null) !== shown) throw new Error(`${kind}: expected ${shown ? "shown" : "suppressed"}, got ${JSON.stringify(e)}`);
      if (!shown && e.suppressed !== "prefs") throw new Error(`${kind} suppressed by ${e.suppressed}, not the prefs`);
    }
  };
  await round({ needs: true, finished: false }, { NeedsYou: true, Finished: false });
  await round({ needs: false, finished: true }, { NeedsYou: false, Finished: true });
  await setPrefs({ "Needs you": true, "A session finished": true });
});

scenario_("app-kill-recovery", async () => {
  // kill -9 the desktop app mid-turn (a gate pending), start it again: the session still
  // runs, its transcript is continuous and the gate is answerable.
  const s = launched.codex;
  if (!s) return "no session";
  await openSession(s);
  await sendPrompt("please run the tests");
  await waitPhase(s.id, "needs_you");
  const daemon = (await rpc("daemon.info")).pid;
  const pids = execFileSync("pgrep", ["-f", bin.app]).toString().trim().split(/\s+/).map(Number);
  for (const p of pids) process.kill(p, "SIGKILL");
  await wd.quit().catch(() => {});
  await startApp();
  if ((await rpc("daemon.info")).pid !== daemon) throw new Error("the daemon was restarted with the app");
  await openSession(s);
  await el("[data-gate=permission]", null, 30_000);
  // The transcript is continuous: one ordered event sequence from the spawn and first prompt to the
  // pending gate, and the UI shows the latest turn above the gate.
  const all = [];
  for (let before = null; ;) {
    const page = await rpc("session.history", { id: s.id, limit: 500, ...(before ? { before } : {}) }, local);
    all.unshift(...page.events);
    if (!page.has_more) break;
    before = page.events[0].cursor;
  }
  const unordered = all.filter((e, i) => i > 0 && e.cursor <= all[i - 1].cursor).length;
  const text = JSON.stringify(all);
  if (unordered || all[0].kind !== "spawned" || !text.includes("hello from codex") || !text.includes("gate_requested")) throw new Error(`history: ${all.length} events, ${unordered} out of order, first ${all[0]?.kind}`);
  await bodyHas("please run the tests");
  await click("[data-gate=permission] .choices button", "Allow");
  await waitPhase(s.id, "idle");
  await bodyHas("All 12 auth tests pass.");
});

// ── remote hosts over a real sshd on localhost ──────────────────────────────

/** Type into the input a <label> names (TextField). */
async function field(label, text) {
  const id = await waitFor(() => wd.exec((l) => {
    const lab = [...document.querySelectorAll("[role=dialog] label")].find((x) => x.textContent.trim() === l);
    return lab?.htmlFor || null;
  }, label), `field ${label}`);
  await type(`[id="${id}"]`, text);
}

const sshd = { up: false };
function startSshd() {
  if (sshd.up) return true;
  if (process.getuid?.() !== 0 || !fs.existsSync("/usr/sbin/sshd")) return false;
  const d = path.join(root, "ssh");
  fs.mkdirSync(path.join(d, "bin"), { recursive: true });
  const rh = path.join(root, "remote-home");
  fs.mkdirSync(rh, { recursive: true });
  for (const n of ["claude", "codex", "opencode", "kilo"]) fs.symlinkSync(bin.fake, path.join(d, "bin", n));
  for (const k of ["hostkey", "id"]) execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", path.join(d, k)]);
  const pub = fs.readFileSync(path.join(d, "id.pub"), "utf8").trim();
  const PATH = `${path.join(d, "bin")}:/usr/local/bin:/usr/bin:/bin`;
  fs.writeFileSync(path.join(d, "authorized_keys"), `environment="HOME=${rh}",environment="PATH=${PATH}",environment="WASIT_FAKE_SCENARIO=${scenarioPath}" ${pub}\n`);
  sshd.port = 20000 + Math.floor(Math.random() * 20000);
  fs.writeFileSync(path.join(d, "sshd_config"), [
    `Port ${sshd.port}`, "ListenAddress 127.0.0.1", `HostKey ${path.join(d, "hostkey")}`, `AuthorizedKeysFile ${path.join(d, "authorized_keys")}`,
    "PermitUserEnvironment yes", "StrictModes no", "PasswordAuthentication yes", "KbdInteractiveAuthentication no", "UsePAM no",
    "PermitRootLogin prohibit-password", `PidFile ${path.join(d, "sshd.pid")}`, "LogLevel ERROR", "",
  ].join("\n"));
  fs.mkdirSync("/run/sshd", { recursive: true });
  // A user that signs in with a password (the sign-in dialog path).
  sshd.pwUser = "marsade2e";
  sshd.password = "walnut-" + Math.random().toString(36).slice(2, 8);
  spawnSync("useradd", ["-m", "-s", "/bin/bash", sshd.pwUser], { stdio: "ignore" });
  execFileSync("chpasswd", { input: `${sshd.pwUser}:${sshd.password}\n` });
  const hostpub = fs.readFileSync(path.join(d, "hostkey.pub"), "utf8").trim().split(/\s+/);
  sshd.knownHosts = path.join(d, "known_hosts");
  fs.writeFileSync(sshd.knownHosts, `[127.0.0.1]:${sshd.port} ${hostpub[0]} ${hostpub[1]}\n`);
  sshd.key = path.join(d, "id");
  sshd.child = spawn("/usr/sbin/sshd", ["-D", "-e", "-f", path.join(d, "sshd_config")], { stdio: ["ignore", "ignore", fs.openSync(path.join(d, "sshd.log"), "a")] });
  sshd.up = true;
  return true;
}

async function addHostViaDialog({ name, target, identity }) {
  await go("#/hosts");
  await button("Add host");
  await el("[role=dialog]");
  await field("Name", name);
  await field("SSH target", target);
  await field("Port", String(sshd.port));
  if (identity) await field("Identity file", identity);
  await field("Extra ssh options", `UserKnownHostsFile=${sshd.knownHosts}`);
  await click("[role=dialog] button.primary", "Add and connect");
}
const hostEntry = async (name) => (await rpc("hosts.list")).hosts.find((h) => h.name === name);
const linkOf = async (name) => (await state())?.hosts?.[(await hostEntry(name))?.id]?.link;

scenario_("ssh-host-key", async () => {
  if (!startSshd()) return "skipped: needs root and sshd";
  await addHostViaDialog({ name: "build-box", target: "root@127.0.0.1", identity: sshd.key });
  // Provisioning uploads this build's marsad-agent, then the link comes up.
  await waitFor(async () => (await linkOf("build-box"))?.state?.state === "connected", "build-box connected", 180_000);
  await click("[role=dialog] button", "Done", 10_000).catch(() => resetUi());
  await shot("40-hosts");
  const h = await hostEntry("build-box");
  // A session on the remote host, launched from the dialog.
  await go("#/inbox");
  await button("New session");
  await el("[role=dialog]");
  await click("[role=dialog] [role=radiogroup][aria-label=Host] [role=radio], [role=dialog] .hostpick button", "build-box", 30_000).catch(() => {});
  await resetUi();
  const r = await rpc("sessions.launch", { host: h.id, driver: "codex", cwd: null, model: null, mode: null, first_prompt: "hello from build-box", resume: { kind: "new" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } });
  await waitFor(async () => (await state())?.sessions?.[`${h.id}/${r.id}`]?.phase?.state === "idle", "remote session idle", 60_000);
  // Kill the link's ssh process: it reconnects with backoff and the session catches up.
  spawnSync("pkill", ["-f", `ssh.*-p ${sshd.port}|ssh.*Port=${sshd.port}`]);
  spawnSync("pkill", ["-f", `${sshd.port}.*bridge`]);
  await waitFor(async () => ["reconnecting", "connecting", "provisioning"].includes((await linkOf("build-box"))?.state?.state), "link dropped", 30_000).catch(() => {});
  await waitFor(async () => (await linkOf("build-box"))?.state?.state === "connected", "build-box reconnected", 120_000);
  await go(`#/session/${encodeURIComponent(h.id)}/${r.id}`);
  await bodyHas("echo: hello from build-box");
});

scenario_("ssh-host-password", async () => {
  if (!startSshd()) return "skipped: needs root and sshd";
  await addHostViaDialog({ name: "lab-pc", target: `${sshd.pwUser}@127.0.0.1` });
  // ssh asks for the password through Marsad's askpass: the sign-in dialog shows its prompt
  // (from the add-host progress, or after "Connect" if the first attempt was left unanswered).
  let sawPrompt = null;
  const labPrompt = async () => Object.values((await state())?.hosts ?? {}).find((x) => x.entry.name === "lab-pc")?.authPrompt ?? null;
  const first = await waitFor(async () => {
    sawPrompt = sawPrompt ?? (await labPrompt());
    if (await wd.exec(() => !!document.querySelector(".authp-input"))) return "prompt";
    return (await linkOf("lab-pc"))?.state?.state === "auth_needed" ? "parked" : null;
  }, "a sign-in prompt", 90_000);
  if (first === "parked") {
    await resetUi();
    await go("#/hosts");
    await click(".hrow[data-host]", "lab-pc");
    if (sawPrompt) console.log(`    note: prompt ${JSON.stringify(sawPrompt)} was published but no dialog input appeared`);
    await click("button.primary", "Connect", 20_000);
  }
  await el(".authp-input", null, 60_000);
  await shot("41-host-sign-in");
  await type(".authp-input", sshd.password);
  await button("Sign in");
  await waitFor(async () => (await linkOf("lab-pc"))?.state?.state === "connected", "lab-pc connected", 180_000);
  await resetUi();
  // A host that needed a password never reconnects on its own: it parks in auth_needed.
  spawnSync("pkill", ["-f", `${sshd.pwUser}@127.0.0.1`]);
  await waitFor(async () => (await linkOf("lab-pc"))?.state?.state === "auth_needed", "lab-pc auth_needed", 60_000);
  await go("#/hosts");
  await click(".hrow[data-host]", "lab-pc").catch(() => {});
  await shot("42-host-auth-needed");
  await click("button.primary", "Connect", 20_000);
  await el(".authp-input", null, 60_000);
  await type(".authp-input", sshd.password);
  await button("Sign in");
  await waitFor(async () => (await linkOf("lab-pc"))?.state?.state === "connected", "lab-pc connected again", 120_000);
});

// Live CLIs: tiny prompts, cheapest models. Each driver: launch, every one of its own gate
// choices, interrupt, model + mode switch, a slash command, and continue-latest / resume-by-id
// of conversations started in a terminal, outside Marsad.
const LIVE = {
  claude: { agent: "Claude Code", model: "Haiku", cliModel: "haiku" },
  opencode: { agent: "OpenCode", model: "opencode/deepseek-v4.1-flash-free", cliModel: "opencode/deepseek-v4.1-flash-free" },
  kilo: { agent: "Kilo Code", model: "kilo/kilo-auto/free", cliModel: "kilo/kilo-auto/free" },
};
const pendingGate = async (id) => (await snapOf(id))?.pending_gates?.[0]?.gate ?? null;
/** Wait until the turn ends; answer any further gate with `kind` (a model may ask again). */
async function settle(id, { answer = null, ms = 180_000 } = {}) {
  const seen = [];
  await waitFor(async () => {
    const s = await snapOf(id);
    if (!s) return false;
    if (s.phase.state === "needs_you") {
      const g = s.pending_gates?.[0]?.gate;
      if (g && !seen.includes(g.request_id)) {
        seen.push(g.request_id);
        if (!answer) throw new Error(`unexpected gate ${g.gate_type}`);
        const c = g.choices.find((x) => x.kind === answer) ?? g.choices.find((x) => x.kind.startsWith(answer.split("_")[0]));
        await openSession({ id });
        await click(`[data-gate] .choices button[data-choice="${c.id}"]`);
      }
      return false;
    }
    return ["idle", "exited", "failed"].includes(s.phase.state);
  }, `session ${id} settled`, ms);
  return seen.length;
}
const shell = (d, cmd) => (d === "claude" ? `Use the Bash tool to run exactly this command and nothing else, then reply done: ${cmd}` : `Use the bash tool to run exactly this command and nothing else, then reply done: ${cmd}`);

for (const [d, L] of Object.entries(LIVE)) {
  scenario_(`live-${d}`, async () => {
    // OpenCode and Kilo run shell commands without asking by default; the project's own
    // config makes them ask (removed again after the interrupt scenario: a session keeps
    // the config it started with, and terminal runs must not wait for an answer).
    if (d !== "claude") fs.writeFileSync(path.join(project, "opencode.json"), JSON.stringify({ permission: { bash: "ask" } }));
    const s = await launchViaDialog({ agent: L.agent, cwd: project, prompt: "Reply with exactly the word: pong", model: L.model });
    launched[`live-${d}`] = s;
    await waitPhase(s.id, "idle", 180_000);
    await openSession(s);
    await bodyHas("pong", 60_000);
  }, { real: true });

  scenario_(`live-${d}-gate-choices`, async () => {
    const s = launched[`live-${d}`];
    if (!s) return "no session";
    await openSession(s);
    // The choices are the CLI's own; answer each in turn (refusals first, "always" last).
    await sendPrompt(shell(d, `touch gate_${d}_0.txt`));
    await waitPhase(s.id, "needs_you", 120_000);
    const first = await pendingGate(s.id);
    if (first?.gate_type !== "permission") throw new Error(`expected a permission gate, got ${first?.gate_type}`);
    const order = ["reject_once", "cancel", "allow_once", "allow_always"];
    const choices = [...first.choices].filter((c) => c.kind !== "reject_always").sort((a, b) => order.indexOf(a.kind) - order.indexOf(b.kind));
    if (d === "claude") await shot("16-session-live-claude-gate");
    const done = [];
    for (const [i, c] of choices.entries()) {
      const f = `gate_${d}_${i}.txt`;
      if (i > 0) {
        await sendPrompt(shell(d, `touch ${f}`));
        await waitPhase(s.id, "needs_you", 120_000);
      }
      const g = await pendingGate(s.id);
      const mine = g.choices.find((x) => x.kind === c.kind) ?? c;
      await click(`[data-gate=permission] .choices button[data-choice="${mine.id}"]`);
      // A refused model may try again: refuse that too.
      await settle(s.id, { answer: c.kind.startsWith("allow") ? "allow_once" : "reject_once" });
      const ran = fs.existsSync(path.join(project, f));
      if (ran !== c.kind.startsWith("allow")) throw new Error(`${c.kind} (${mine.label}): command ${ran ? "ran" : "did not run"}`);
      done.push(`${mine.label} [${c.kind}]`);
    }
    // After "always", the same command runs without asking.
    if (choices.some((c) => c.kind === "allow_always")) {
      const f = `gate_${d}_${choices.length - 1}.txt`;
      fs.rmSync(path.join(project, f), { force: true });
      await sendPrompt(shell(d, `touch ${f}`));
      const asked = await settle(s.id, { answer: "allow_once" });
      if (asked) throw new Error("asked again after an always-allow choice");
      if (!fs.existsSync(path.join(project, f))) throw new Error("always-allowed command did not run");
    }
    const skipped = first.choices.filter((c) => c.kind === "reject_always").map((c) => c.label);
    return `answered ${done.join(", ")}${skipped.length ? `; not exercised: ${skipped}` : ""}`;
  }, { real: true });

  scenario_(`live-${d}-interrupt`, async () => {
    const s = launched[`live-${d}`];
    if (!s) return "no session";
    await openSession(s);
    await sendPrompt(shell(d, `python3 -c "import time; time.sleep(120)"`));
    await waitFor(async () => {
      const x = await snapOf(s.id);
      if (x.phase.state === "needs_you") {
        const g = x.pending_gates[0].gate;
        await click(`[data-gate=permission] .choices button[data-choice="${g.choices.find((c) => c.kind === "allow_once").id}"]`);
        return false;
      }
      return x.phase.state === "working" && x.turn?.current_tool;
    }, "the sleep running", 120_000).catch(async (e) => {
      // A model that started a second tool call next to the sleep leaves no "current" tool.
      if ((await snapOf(s.id)).phase.state !== "working") throw e;
    });
    await sleep(2000);
    await button("Interrupt");
    await waitPhase(s.id, "idle", 60_000);
    fs.rmSync(path.join(project, "opencode.json"), { force: true });
  }, { real: true });

  scenario_(`live-${d}-model-mode`, async () => {
    const s = launched[`live-${d}`];
    if (!s) return "no session";
    const x = await snapOf(s.id);
    // claude lists aliases ("sonnet") and reports the resolved name ("claude-sonnet-…").
    const same = (cur, id) => cur === id || (cur ?? "").includes(id);
    const model = x.available_models.find((m) => !same(x.model, m.id) && m.id !== "default" && !/opus/i.test(m.id) && (d === "claude" || /free/.test(m.id)));
    const mode = x.available_modes.find((m) => m.id !== x.mode);
    if (!model || !mode) throw new Error(`nothing to switch to (${x.available_models.length} models, ${x.available_modes.length} modes)`);
    const pick = async (what, label) => {
      await click(`button[aria-label^='${what}:']`);
      const filter = await wd.findAll("css selector", `input[aria-label='Filter ${what}']`);
      if (filter.length) await wd.sendKeys(filter[0], label);
      await click("[role=option]", label);
    };
    await openSession(s, "config");
    await pick("Model", model.display_name);
    await pick("Mode", mode.label);
    await button("Apply to running session");
    await waitFor(async () => { const y = await snapOf(s.id); return same(y.model, model.id) && y.mode === mode.id; }, `model ${model.id} and mode ${mode.id} applied`, 60_000);
    // And back, so the next scenarios run as before.
    await openSession(s, "config");
    await pick("Model", x.available_models.find((m) => same(x.model, m.id) && m.id !== "default")?.display_name ?? x.model);
    // claude reports "default" for the mode its --help calls "manual".
    const back = x.available_modes.find((m) => m.id === x.mode) ?? x.available_modes.find((m) => m.id === "manual");
    if (back) await pick("Mode", back.label);
    await button("Apply to running session");
    await waitFor(async () => { const y = await snapOf(s.id); return same(y.model, x.model) && (!back || y.mode === back.id || (back.id === "manual" && y.mode === "default")); }, "switched back", 60_000);
    return `${model.id} / ${mode.id} and back`;
  }, { real: true });

  scenario_(`live-${d}-slash`, async () => {
    const s = launched[`live-${d}`];
    if (!s) return "no session";
    const x = await snapOf(s.id);
    const names = x.slash_commands.map((c) => c.name);
    const name = ["context", "cost", "status", "review", "init"].find((n) => names.includes(n)) ?? names[0];
    if (!name) throw new Error("the CLI reported no slash commands");
    const before = x.turns_completed;
    await openSession(s);
    await type("textarea[aria-label=Message]", `/${name}`);
    await el("[role=listbox][aria-label='Slash commands']");
    await wd.sendKeys(await el("textarea[aria-label=Message]"), Keys.ENTER);
    await settle(s.id, { answer: "reject_once" });
    await waitFor(async () => (await snapOf(s.id)).turns_completed > before, `/${name} completed a turn`, 120_000);
    return `/${name} of ${names.length}`;
  }, { real: true });

  scenario_(`live-${d}-continue-and-resume`, async () => {
    // Two conversations started in a terminal, outside Marsad: continue the latest (B), then
    // resume the older one (A) by its id.
    // OpenCode / Kilo take the project from $PWD. A free model sometimes stalls a run: retry.
    const cliEnv = { ...process.env, ...userXdg, PWD: project };
    const cli = (word) => {
      const prompt = `Remember the word ${word}. Reply with just: ok`;
      for (let attempt = 1; ; attempt++) {
        try {
          if (d === "claude") return JSON.parse(execFileSync("claude", ["-p", prompt, "--model", L.cliModel, "--output-format", "json"], { cwd: project, stdio: "pipe", timeout: 180_000, env: cliEnv }).toString()).session_id;
          const out = execFileSync(d, ["run", "--format", "json", "-m", L.cliModel, prompt], { cwd: project, stdio: "pipe", timeout: 90_000, env: cliEnv }).toString();
          return JSON.parse(out.split("\n").find((l) => l.includes("sessionID"))).sessionID;
        } catch (e) {
          if (attempt >= 3) throw e;
          console.log(`    note: ${d} run attempt ${attempt} failed (${e.message.split("\n")[0]}); retrying`);
        }
      }
    };
    const a = cli("walnut");
    await sleep(1500);
    const b = cli("saffron");
    await rpc("drivers.refresh", { driver: d, project }, local);
    await waitFor(async () => (await rpc("drivers.inventory", { project }, local)).drivers.find((x) => x.driver === d && !x.refreshing)?.sessions.some((x) => x.id === b), "the terminal sessions listed", 120_000);
    const ask = "Which word did I ask you to remember? Reply with that one word.";
    const l = await launchViaDialog({ agent: L.agent, cwd: project, prompt: ask, latest: true, model: L.model });
    await waitPhase(l.id, "idle", 180_000);
    await openSession(l);
    await bodyHas("saffron", 60_000);
    const r = await launchViaDialog({ agent: L.agent, cwd: project, prompt: ask, resumeId: a, model: L.model });
    await waitPhase(r.id, "idle", 180_000);
    if ((await snapOf(r.id)).native_session_id !== a && d !== "claude") throw new Error(`resumed ${await snapOf(r.id).native_session_id}, not ${a}`);
    await openSession(r);
    await bodyHas("walnut", 60_000);
  }, { real: true });
}

scenario_("live-claude-config-override", async () => {
  // An MCP server claude inherits from its own config (local scope for this project) is
  // disabled by an override and another one is added; the launched CLI's own report (its
  // init message's server list) must agree.
  const server = (name) => ({ command: bin.agent, args: ["mcp", "--socket", path.join(root, "none.sock"), "--session", "aaaaaaaaaaaa"] });
  const claudeEnv = { ...process.env, ...userXdg, PWD: project };
  execFileSync("claude", ["mcp", "add-json", "--scope", "local", "inherited-tools", JSON.stringify(server())], { cwd: project, stdio: "pipe", env: claudeEnv });
  try {
    await rpc("drivers.refresh", { driver: "claude", project }, local);
    const base = await waitFor(async () => { const b = await rpc("agentconfig.base", { driver: "claude", project, refresh: false }, local); return !b.refreshing && b; }, "claude base layer", 120_000);
    if (!base.items.some((i) => i.kind === "mcp_server" && i.name === "inherited-tools")) throw new Error("the inherited server is not in the base layer");
    const t = server();
    const r = await rpc("sessions.launch", {
      host: local, driver: "claude", cwd: project, model: "haiku", mode: null, first_prompt: "Reply with exactly: ready", resume: { kind: "new" }, labels: {}, workstream: null,
      overrides: { items: [
        { kind: "mcp_server", name: "inherited-tools", driver: null, action: "disable" },
        { kind: "mcp_server", name: "added-tools", driver: null, action: "set", value: { kind: "mcp_server", server: { transport: "stdio", command: t.command, args: t.args, env: {} } } },
      ], env: [], disallowed_tools: [] },
    });
    await waitPhase(r.id, "idle", 180_000);
    const page = await rpc("session.history", { id: r.id, limit: 500 }, local);
    const init = page.events.find((e) => e.kind === "agent" && e.event.type === "session_init");
    const names = (init?.event.mcp_servers ?? []).map((m) => m.name);
    if (!names.includes("added-tools") || names.includes("inherited-tools") || !names.includes("wasl")) throw new Error(`claude reports MCP servers ${JSON.stringify(names)}`);
    await openSession({ id: r.id }, "config");
    await shot("18-session-config-overrides");
    return `claude reports ${names.join(", ")}`;
  } finally {
    spawnSync("claude", ["mcp", "remove", "--scope", "local", "inherited-tools"], { cwd: project, stdio: "ignore", env: claudeEnv });
  }
}, { real: true });

// OpenCode / Kilo cannot ask through their own question tool; Marsad injects `ask_user`.
for (const d of ["opencode", "kilo"]) {
  scenario_(`live-${d}-ask-user`, async () => {
    const s = launched[`live-${d}`];
    if (!s) return "no session";
    const x = await snapOf(s.id);
    await openSession(s);
    await sendPrompt("Use the ask_user tool (from the marsad MCP server) to ask me which color I prefer, as a single choice between the options Red and Blue. After I answer, reply with only the color I picked, in capital letters.");
    await waitFor(async () => (await pendingGate(s.id))?.gate_type === "question", "an ask_user question", 180_000);
    await el("[data-gate=question]");
    if (d === "opencode") await shot("17-session-ask-user-question-opencode");
    await click("[data-gate=question] [role=radio]", "Blue");
    await button("Send answer");
    await waitFor(async () => { const y = await snapOf(s.id); return y.phase.state === "idle" && y.turns_completed > x.turns_completed; }, "the turn after the answer", 180_000);
    await bodyHas("BLUE", 30_000);
  }, { real: true });
}

scenario_("live-delegation", async () => {
  // A claude orchestrator delegates to an opencode worker through the injected Wasl MCP tools;
  // the handoff gate is answered in the UI.
  const node = (id, name, driver, model, prompt) => ({ id, name, role: id, description: `the ${name}`, session: null, launch: { host: null, driver, cwd: project, model, mode: null, first_prompt: prompt, labels: {}, overrides: { items: [], env: [], disallowed_tools: [] } } });
  const ws = await rpc("workstreams.create", {
    name: "Live delegation", description: "claude orchestrator, opencode worker",
    nodes: [node("orch", "orchestrator", "claude", "haiku", null), node("worker", "worker", "opencode", LIVE.opencode.model, null)],
    edges: [{ from: "orch", to: "worker", policy: "ask" }], limits: {}, layout: {},
  });
  await go(`#/workstreams/${ws.id}`);
  await button("Launch");
  const v = await waitFor(async () => { const x = await rpc("workstreams.get", { id: ws.id }); return x.nodes.every((n) => n.session) && x; }, "nodes launched", 120_000);
  const orch = v.nodes.find((n) => n.id === "orch").session;
  await waitPhase(orch.session, ["idle"], 120_000);
  await go(`#/session/${encodeURIComponent(orch.host)}/${orch.session}`);
  await el(".sd-head");
  await sendPrompt("Use your wasl delegate tool to send the node named worker this instruction: 'Reply with exactly the word kiwi.' Wait for its result, then tell me the word it answered.");
  await el("[data-gate=handoff]", null, 180_000);
  await shot("33-workstream-live-handoff-gate");
  await click("[data-gate=handoff] .choices button", "Allow once");
  await waitFor(async () => { const s = await snapOf(orch.session); return s?.phase.state === "idle" && s.turns_completed >= 1; }, "the orchestrator finished", 300_000);
  await bodyHas("kiwi", 60_000);
}, { real: true });

// ── screenshots for the owner (--shots): every screen on believable live data ─────────────

const key = (k, mods = {}) => wd.exec((k, m) => document.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...m })), k, mods);
const launchRpc = (host, driver, prompt, extra = {}) => rpc("sessions.launch", { host, driver, cwd: extra.cwd ?? null, model: null, mode: null, first_prompt: prompt, resume: { kind: "new" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } });
const sessionPage = async (host, id, tab = "") => { await go(`#/session/${encodeURIComponent(host)}/${id}${tab ? `?tab=${tab}` : ""}`); await el(".sd-head"); };

scenario_("showcase", async () => {
  if (!dirs.shots) return "skipped: no --shots";
  const drv = REAL ? "codex" : "claude"; // a fake persona on this host
  // Local sessions: finished unseen, failed, working, gates.
  const finished = await launchRpc(local, drv, "migrate the session cookies to signed tokens", { cwd: project });
  const failed = await launchRpc(local, "codex", "deploy the billing service to staging", { cwd: project });
  const working = await launchRpc(local, "codex", "start the long task: refactor the session store", { cwd: project });
  const gated = await launchRpc(local, "codex", "please run the tests for the auth module", { cwd: project });
  await waitPhase(failed.id, ["failed", "exited"], 60_000);
  await waitPhase(gated.id, "needs_you", 60_000);
  // Remote sessions on build-box (fake personas of every driver).
  const box = await hostEntry("build-box");
  const remote = [];
  if (box) {
    remote.push(await launchRpc(box.id, "claude", "please run the tests on the build box"));
    remote.push(await launchRpc(box.id, "opencode", "which approach for the nightly build cache?"));
    remote.push(await launchRpc(box.id, "kilo", "migrate the session cookies on the build box"));
  }
  // A running three-node workstream: one task in flight, one handoff waiting for you.
  const wnode = (id, name, role, host) => ({ id, name, role, description: `the ${role}`, session: null, launch: { host, driver: "codex", cwd: host === local ? project : null, model: null, mode: null, first_prompt: null, labels: {}, overrides: { items: [], env: [], disallowed_tools: [] } } });
  const ws = await rpc("workstreams.create", {
    name: "Invoice export v3", description: "An orchestrator with a PDF and a CSV worker",
    nodes: [wnode("orch", "orchestrator", "lead", local), wnode("pdf", "pdf-worker", "PDF rendering", local), wnode("csv", "csv-worker", "CSV export", box ? box.id : local)],
    edges: [{ from: "orch", to: "pdf", policy: "ask" }, { from: "orch", to: "csv", policy: "ask" }], limits: {}, layout: {},
  });
  await rpc("workstreams.launch", { id: ws.id });
  const v = await waitFor(async () => { const x = await rpc("workstreams.get", { id: ws.id }); return x.nodes.every((n) => n.session) && x; }, "showcase nodes launched", 120_000);
  const orch = v.nodes.find((n) => n.id === "orch").session;
  await sleep(1500);
  await rpc("wasl.delegate", { from: orch.session, to: ["pdf-worker"], mode: "command", instruction: "render the PDF export for INV-2041" }, orch.host);
  await click("[data-gate=handoff] .choices button", "Allow once", 30_000).catch(async () => {
    await go(`#/session/${encodeURIComponent(orch.host)}/${orch.session}`);
    await click("[data-gate=handoff] .choices button", "Allow once", 30_000);
  });
  await sleep(800);
  await rpc("wasl.delegate", { from: orch.session, to: ["csv-worker"], mode: "command", instruction: "export the CSV for INV-2041" }, orch.host);
  // A draft for design mode.
  const draft = await rpc("workstreams.create", {
    name: "Release notes pipeline", description: "Collect merged PRs, draft notes, review",
    nodes: [wnode("collect", "collector", "collect merged PRs", local), wnode("write", "writer", "draft the notes", local), wnode("review", "reviewer", "review the draft", local)],
    edges: [{ from: "collect", to: "write", policy: "always_allow" }, { from: "write", to: "review", policy: "ask" }], limits: {}, layout: {},
  });
  // Live gates: claude's own choices, and an ask_user question from OpenCode.
  const lc = launched["live-claude"];
  if (lc) { await openSession(lc); await sendPrompt(shell("claude", "mkdir -p build/reports")); await waitPhase(lc.id, "needs_you", 120_000).catch(() => {}); }
  const lo = launched["live-opencode"];
  if (lo) { await openSession(lo); await sendPrompt("Use the ask_user tool (marsad MCP server) to ask me which export format to ship first, as a single choice between PDF, CSV and XLSX, each with a one-line description. Then reply with my choice."); await waitFor(async () => (await pendingGate(lo.id))?.gate_type === "question", "ask_user question", 120_000).catch(() => {}); }
  await sleep(1500);

  await rpc("store.settings.set", { key: "theme", value: "dark" });
  await go("#/inbox");
  await sleep(1500);
  await shot("dark-01-inbox");
  // Session screens.
  if (lc) { await openSession(lc); await el("[data-gate=permission]", null, 5000).catch(() => {}); await shot("dark-02-session-activity-permission-gate-claude-live"); }
  if (lo) { await openSession(lo); await shot("dark-03-session-question-gate-ask-user-opencode-live"); }
  const main = launched.claude ?? launched.codex ?? finished;
  for (const [tab, name] of [["changes", "dark-04-session-changes"], ["config", "dark-05-session-config"], ["usage", "dark-06-session-usage"], ["terminal", "dark-07-session-terminal"]]) {
    await openSession(finished, tab);
    if (tab === "changes") { fs.appendFileSync(path.join(project, "src/auth.ts"), "export const TOKEN_TTL_DAYS = 30;\n"); await rpc("fleet.git_refresh", { host: local, cwd: project }); await click("[role=option]", "src/auth.ts", 20_000).catch(() => {}); }
    if (tab === "terminal") { await click(".xterm-box .xterm").catch(() => {}); await wd.sendKeys(await el(".xterm-box textarea"), "git log --oneline | head -5" + Keys.ENTER).catch(() => {}); await sleep(1200); }
    await shot(name);
  }
  void main;
  await openSession(failed); await shot("dark-08-session-failed");
  // New session dialog.
  await go("#/inbox");
  await button("New session");
  await el("[role=dialog]");
  await click("[role=dialog] [role=radio]:not([disabled])", "Claude Code", 60_000);
  await waitFor(async () => (await wd.exec(() => document.querySelector("#ns-cwd")?.value)) === project || (await type("#ns-cwd", project), false), "folder", 20_000).catch(() => {});
  await type("#ns-prompt", "Add rate limiting to the invoice API");
  await sleep(800);
  await shot("dark-09-new-session-new");
  await button("Continue an existing one");
  await sleep(1500);
  await shot("dark-10-new-session-continue-resume-list");
  await resetUi();
  // Workstreams.
  await go("#/workstreams"); await sleep(800); await shot("dark-11-workstreams-list");
  await go(`#/workstreams/${ws.id}`); await el("[data-gate=handoff]", null, 20_000).catch(() => {}); await sleep(1200); await shot("dark-12-workstream-graph-running-handoff-pending");
  await go(`#/workstreams/${draft.id}`); await sleep(1200); await shot("dark-13-workstream-design-mode");
  // Hosts, config, settings.
  await go("#/hosts"); await sleep(800); await click(".hrow[data-host]", "build-box").catch(() => {}); await sleep(600); await shot("dark-14-hosts");
  await go("#/config"); await sleep(1500); await shot("dark-15-config-agent-configuration");
  await go("#/settings"); await button("Notifications"); await sleep(600); await shot("dark-16-settings-notifications");
  // Palette and shortcut sheet.
  await go("#/inbox"); await sleep(500);
  await key("k", { ctrlKey: true }); await sleep(700); await shot("dark-17-command-palette"); await key("Escape"); await resetUi();
  await key("?"); await sleep(700); await shot("dark-18-shortcut-sheet"); await key("Escape"); await resetUi();
  // Light.
  await rpc("store.settings.set", { key: "theme", value: "light" });
  await go("#/inbox"); await sleep(1200); await shot("light-01-inbox");
  await go(`#/workstreams/${ws.id}`); await sleep(1200); await shot("light-02-workstream-graph-running");
  await rpc("store.settings.set", { key: "theme", value: "dark" });
  // Leave nothing waiting on a person.
  for (const id of [lc?.id, lo?.id].filter(Boolean)) {
    const g = await pendingGate(id);
    if (g?.gate_type === "permission") await rpc("session.command", { id, command: { command: "answer_gate", request_id: g.request_id, decision: { choice: g.choices.find((c) => c.kind === "reject_once")?.id ?? g.choices[0].id } } }, local).catch(() => {});
    if (g?.gate_type === "question") await rpc("session.command", { id, command: { command: "answer_question", request_id: g.request_id, answer: { answers: { [g.questions[0].id]: ["PDF"] } } } }, local).catch(() => {});
  }
  void working; void remote;
  return `${(await fleetSessions()).length} sessions on this host`;
});

/** Close whatever a failed scenario left open (dialogs, menus) and go to the Inbox. */
async function resetUi() {
  for (let i = 0; i < 3; i++) {
    const open = await wd.exec(() => document.querySelectorAll("[role=dialog], [role=listbox][id]").length);
    if (!open) break;
    await wd.exec(() => document.querySelectorAll("[role=dialog] button[aria-label=Close], [role=dialog] .x, .toast button[aria-label=Dismiss]").forEach((b) => b.click()));
    await wd.exec(() => document.activeElement?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    await sleep(200);
  }
  await go("#/inbox");
}

// ── run ─────────────────────────────────────────────────────────────────────

let code = 0;
try {
  await startApp();
  local = (await rpc("hosts.list")).hosts.find((h) => h.kind.kind === "local").id;
  // Screenshots for the owner are taken in the dark theme (the showcase adds light ones).
  if (dirs.shots) await rpc("store.settings.set", { key: "theme", value: "dark" });
  for (const sc of scenarios) {
    if (sc.real && !REAL) continue;
    if (ONLY && !ONLY.includes(sc.name)) continue;
    const t0 = Date.now();
    try {
      await resetUi();
      const note = await sc.fn();
      results.push({ name: sc.name, ok: true, ms: Date.now() - t0, note });
      console.log(`  ok   ${sc.name} (${Date.now() - t0} ms)${note ? ` — ${note}` : ""}`);
    } catch (e) {
      failed++;
      results.push({ name: sc.name, ok: false, ms: Date.now() - t0, error: e.message });
      console.log(`  FAIL ${sc.name}: ${e.message}`);
      try {
        fs.writeFileSync(path.join(root, `fail-${sc.name}.png`), await wd.screenshot());
      } catch {
        // the window may be gone; the log has the error
      }
    }
  }
} catch (e) {
  failed++;
  console.log(`  FAIL setup: ${e.stack ?? e}`);
} finally {
  await wd.quit().catch(() => {});
  driver.kill();
  sshd.child?.kill();
  if (sshd.pwUser) spawnSync("userdel", ["-r", sshd.pwUser], { stdio: "ignore" });
  spawnSync(bin.agent, ["stop", "--data", dirs.data], { stdio: "ignore" });
  // Sessions outlive the daemon by design (their shims hold them); end this run's ones.
  spawnSync("pkill", ["-f", `${dirs.data}/sessions`], { stdio: "ignore" });
  // The ssh hosts' daemons and sessions live under this run's temp dir too.
  spawnSync("pkill", ["-f", root], { stdio: "ignore" });
  code = failed ? 1 : 0;
  console.log(`\n${results.filter((r) => r.ok).length} passed, ${failed} failed${KEEP || failed ? ` — data in ${root}` : ""}`);
  if (!KEEP && !failed) fs.rmSync(root, { recursive: true, force: true });
}
void state;
void sessionByTitle;
process.exit(code);
