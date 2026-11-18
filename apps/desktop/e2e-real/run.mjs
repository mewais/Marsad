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
async function launchViaDialog({ agent, cwd, prompt, latest = false, resumeTitle, model }) {
  await go("#/inbox");
  await button("New session");
  await el("[role=dialog]");
  // The host's discovery may still be probing the CLIs: wait until this one is offered.
  await click("[role=dialog] [role=radio]:not([disabled])", agent, 120_000);
  await type("#ns-cwd", cwd);
  if (model) {
    await click("[role=dialog] button[aria-label^='Model:']", null, 90_000);
    const filter = await wd.findAll("css selector", "input[aria-label='Filter Model']");
    if (filter.length) await wd.sendKeys(filter[0], model);
    await click("[role=listbox][aria-label=Model] [role=option]", model, 30_000);
  }
  if (latest || resumeTitle) {
    await button("Continue an existing one");
    if (latest) {
      // "Continue most recent" (the CLI's own continue) is the default when the CLI has it.
      const b = await el("[role=dialog] .recent button");
      if ((await wd.text(b)).trim() !== "Selected") await wd.click(b);
    }
    else await click("[role=option]", resumeTitle, 60_000);
  }
  if (prompt) await type("#ns-prompt", prompt);
  const before = new Set((await fleetSessions()).map((s) => s.id));
  await click("[role=dialog] button.primary");
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
  if (REAL) return "covered by live-claude-resume-terminal-session";
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

// Live CLIs: tiny prompts, cheapest models.
const live = (d, model, agent) => scenario_(`live-${d}`, async () => {
  const s = await launchViaDialog({ agent, cwd: project, prompt: "Reply with exactly the word: pong", model });
  launched[`live-${d}`] = s;
  await waitPhase(s.id, "idle", 180_000);
  await openSession(s);
  await bodyHas("pong", 60_000);
}, { real: true });
live("claude", "Haiku", "Claude Code");
live("opencode", "opencode/deepseek-v4.1-flash-free", "OpenCode");
live("kilo", "kilo/kilo-auto/free", "Kilo Code");

scenario_("live-claude-gate", async () => {
  const s = launched["live-claude"];
  if (!s) return "no session";
  await openSession(s);
  await sendPrompt("Use the Bash tool to run exactly: touch live_gate.txt");
  await waitPhase(s.id, "needs_you", 120_000);
  await shot("16-session-live-claude-gate");
  await click("[data-gate=permission] .choices button", "Allow");
  await waitPhase(s.id, "idle", 120_000);
  if (!fs.existsSync(path.join(project, "live_gate.txt"))) throw new Error("the allowed command did not run");
}, { real: true });

scenario_("live-claude-resume-terminal-session", async () => {
  // A conversation started in a terminal, outside Marsad, then resumed from the dialog.
  execFileSync("claude", ["-p", "Remember the word: walnut. Reply ok.", "--model", "haiku"], { cwd: project, stdio: "pipe", timeout: 120_000 });
  await rpc("drivers.refresh", { driver: "claude", project }, local);
  const s = await launchViaDialog({ agent: "Claude Code", cwd: project, prompt: "Which word did I ask you to remember? One word.", resumeTitle: "Remember the word: walnut", model: "Haiku" });
  await waitPhase(s.id, "idle", 180_000);
  await openSession(s);
  await bodyHas("walnut", 60_000);
}, { real: true });

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
