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
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";
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

/** A real PNG: bars on a light background (zlib from node). */
function chartPng(w = 240, h = 150) {
  const bars = [0.35, 0.6, 0.45, 0.8, 0.95, 0.7];
  const raw = Buffer.alloc((w * 3 + 1) * h);
  let i = 0;
  for (let y = 0; y < h; y++) {
    raw[i++] = 0;
    for (let x = 0; x < w; x++) {
      const slot = Math.floor((x / w) * bars.length);
      const col = x % (w / bars.length);
      const bar = col > 6 && col < w / bars.length - 6 && y > h - 12 - bars[slot] * (h - 24) && y < h - 12;
      const [r, g, b] = y === h - 12 || x === 3 ? [60, 60, 70] : bar ? [70 + slot * 25, 120 + slot * 12, 230 - slot * 20] : [246, 247, 250];
      raw[i++] = r; raw[i++] = g; raw[i++] = b;
    }
  }
  const crcT = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  const crc = (b) => { let c = ~0; for (const x of b) c = crcT[(c ^ x) & 255] ^ (c >>> 8); return (~c) >>> 0; };
  const chunk = (t, d) => { const td = Buffer.concat([Buffer.from(t), d]); const out = Buffer.alloc(td.length + 8); out.writeUInt32BE(d.length, 0); td.copy(out, 4); out.writeUInt32BE(crc(td), td.length + 4); return out; };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", zlib.deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

/** A minimal one-page PDF. */
function simplePdf(title, line) {
  const stream = `BT /F1 22 Tf 72 720 Td (${title}) Tj ET BT /F1 12 Tf 72 690 Td (${line}) Tj ET 0.2 0.4 0.9 rg 72 520 120 140 re f 0.9 0.5 0.2 rg 210 520 120 100 re f`;
  const objs = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`];
  let s = "%PDF-1.4\n";
  const offs = [];
  objs.forEach((o, i) => { offs.push(s.length); s += `${i + 1} 0 obj\n${o}\nendobj\n`; });
  const xref = s.length;
  s += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n${offs.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("")}trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return s;
}

const project = makeRepo("ledgerline", {
  "assets/sketch.png": chartPng(),
  "report.pdf": simplePdf("Quarterly report", "Revenue grew 45% in Q4."),
  "preview.html": "<h1>Quarterly report</h1><p>Generated by the agent.</p><img src='https://example.com/t.png' alt='remote'><script>document.querySelector('h1').textContent='Scripts ran'</script>",
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
    // The fake agent spawns a worker through its injected Wasl MCP tool, waits for its first
    // task and reports the answer (the spawned worker plays this same scenario).
    { when: "spawn a worker", steps: [
      { type: "say", text: "I'll start a worker for the arithmetic." },
      { type: "mcp_call", server: "wasl", tool: "spawn_agent", arguments: { name: "calc", role: "Does the arithmetic", instruction: "what is 6*7?" }, save_as: "s" },
      { type: "mcp_call", server: "wasl", tool: "task_wait", arguments: { ids: ["{{s.task.id}}"], timeout_secs: 120 }, save_as: "w" },
      { type: "say", text: "calc answered: {{w.tasks.0.result}}" },
    ] },
    { when: "what is 6\\*7", steps: [{ type: "say", text: "42" }] },
    // Media: a tool result with an image (like a Read of a PNG) and a file the agent writes.
    { when: "draw a chart", steps: [
      { type: "tool", name: "Read", input: { file_path: "assets/sketch.png" }, needs_permission: false, image: { mime: "image/png", path: "assets/sketch.png" } },
      { type: "tool", name: "Write", input: { file_path: "out/chart.svg" }, needs_permission: false, result_if_allowed: "File created successfully at: out/chart.svg",
        write_file: { path: "out/chart.svg", content: "<svg xmlns='http://www.w3.org/2000/svg' width='320' height='200'><rect width='320' height='200' fill='#f6f7fa'/><g fill='#4a78e6'><rect x='30' y='110' width='40' height='70'/><rect x='90' y='70' width='40' height='110'/><rect x='150' y='90' width='40' height='90'/><rect x='210' y='40' width='40' height='140'/></g><script>document.title='pwned'</script></svg>" } },
      { type: "image", mime: "image/png", path: "assets/sketch.png" },
      { type: "say", text: "Drew `out/chart.svg` from your sketch; the write-up is in report.pdf and preview.html shows both." },
    ] },
    // Polish: an HTML file with a relative image, a code file, a Markdown file, an edit (diff) and
    // a reply with a fenced block and path mentions (one inside the folder with a line, one outside).
    { when: "write the polish files", steps: [
      { type: "tool", name: "Write", input: { file_path: "polish/site/index.html" }, needs_permission: false, result_if_allowed: "File created successfully at: polish/site/index.html",
        write_file: { path: "polish/site/index.html", content: "<!doctype html><html><head><style>body{font-family:sans-serif;background:#f6f7fa}</style></head><body><h1 id='t'>Polish report</h1><img id='sk' src='../../assets/sketch.png' alt='sketch' width='96'><img src='https://example.com/tracker.png' alt='remote'><img src='../../../../../../etc/passwd.png' alt='escape'></body></html>" } },
      { type: "tool", name: "Write", input: { file_path: "polish/src/app.ts" }, needs_permission: false, result_if_allowed: "File created successfully at: polish/src/app.ts",
        write_file: { path: "polish/src/app.ts", content: "import { readFile } from 'node:fs/promises';\n\n/** Sum the rows. */\nexport async function total(path: string): Promise<number> {\n  const rows: number[] = JSON.parse(await readFile(path, 'utf8'));\n  return rows.reduce((a, b) => a + b, 0); // done\n}\n" } },
      { type: "tool", name: "Write", input: { file_path: "polish/docs/guide.md" }, needs_permission: false, result_if_allowed: "File created successfully at: polish/docs/guide.md",
        write_file: { path: "polish/docs/guide.md", content: "# Guide\n\nRun it:\n\n```bash\nnpm test -- --watch\n```\n\n- [x] install\n- [ ] ship\n\n| a | b |\n|---|---|\n| 1 | 2 |\n" } },
      { type: "tool", name: "Edit", input: { file_path: "polish/src/app.ts", old_string: "return rows.reduce((a, b) => a + b, 0);", new_string: "return rows.length ? rows.reduce((a, b) => a + b, 0) : 0;" }, needs_permission: false, result_if_allowed: "Edited polish/src/app.ts" },
      { type: "say", text: "Done:\n\n```ts\nexport const total = (rows: number[]) => rows.reduce((a, b) => a + b, 0);\n```\n\nSee `polish/src/app.ts:2`, `polish/site/index.html` and `polish/docs/guide.md`; not `../../etc/passwd`." },
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

/** The UI theme is a per-window preference (localStorage), applied on load. */
async function setTheme(t) {
  await wd.exec((t) => { localStorage.setItem("marsad.theme", t); location.reload(); }, t).catch(() => {});
  await waitFor(() => wd.exec((t) => document.documentElement.getAttribute("data-theme") === t && !!document.querySelector(".ibx-item, .placeholder, .empty, .sd-head, main"), t), `theme ${t}`, 30_000);
}

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
async function launchViaDialog({ agent, cwd, prompt, latest = false, resumeTitle, resumeId, model, attach, beforeStart }) {
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
  if (attach) {
    // Drop an image on the first-prompt field, as a user would.
    await waitFor(() => wd.exec(() => !document.querySelector("[aria-label='Attach files']")?.disabled), "attach enabled", 30_000);
    await wd.exec((b64) => {
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      const dt = new DataTransfer();
      dt.items.add(new File([bytes], "sketch.png", { type: "image/png" }));
      const target = document.querySelector(".ns-prompt");
      target.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
      target.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
    }, attach);
    await el("[role=dialog] [data-testid=pending-attachment]", "sketch.png");
  }
  await beforeStart?.();
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

// Images and files: an attachment dropped on the composer is uploaded and sent; the tool's
// image shows inline (from the host's blob store); files the agent wrote or mentions are chips
// that open the preview (SVG as an image, PDF through pdf.js, HTML sandboxed without scripts).
scenario_("media", async () => {
  const s = launched.claude;
  if (!s) return "no session";
  await openSession(s);
  await wd.exec(() => document.querySelectorAll(".toast button[aria-label=Dismiss]").forEach((b) => b.click()));
  const png = fs.readFileSync(path.join(project, "assets/sketch.png")).toString("base64");
  await wd.exec((b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const dt = new DataTransfer();
    dt.items.add(new File([bytes], "sketch.png", { type: "image/png" }));
    const target = document.querySelector(".composer");
    target.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
    target.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, png);
  await el("[data-testid=pending-attachment]", "sketch.png");
  await type("textarea[aria-label=Message]", "please draw a chart like this sketch");
  await shot("media-composer");
  await button("Send");
  await waitPhase(s.id, "idle");
  await bodyHas("Drew out/chart.svg");
  // The sent attachment and the tool / reply images are real images from the host.
  await waitFor(() => wd.exec(() => [...document.querySelectorAll("[data-testid=media-image] img")].filter((i) => i.complete && i.naturalWidth > 0).length >= 2), "attachment and tool image loaded", 30_000);
  const att = await rpc("session.file.read", { id: s.id, path: "assets/sketch.png", offset: 0, length: 16 }, local);
  if (att.mime !== "image/png") throw new Error(`sketch read as ${att.mime}`);
  await wd.exec(() => document.querySelector(".transcript").scrollTo(0, 1e9));
  await shot("media-transcript");
  await click("[data-testid=file-ref] button", "chart.svg");
  await waitFor(() => wd.exec(() => document.querySelector("[data-testid=preview]")?.getAttribute("data-kind") === "svg" && document.querySelector("[data-testid=preview-image]")?.complete), "svg preview", 20_000);
  if ((await wd.exec(() => document.title)) === "pwned") throw new Error("the SVG's script ran");
  await shot("media-chips");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await click("[data-testid=media-image]");
  await el("[data-testid=preview-image]");
  await shot("media-lightbox");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await click("[data-testid=file-ref] button", "report.pdf");
  await waitFor(() => wd.exec(() => document.querySelectorAll(".pv-pages canvas").length === 1), "pdf page rendered", 30_000);
  await shot("media-pdf");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await click("[data-testid=file-ref] button", "preview.html");
  await el("[data-testid=preview-html]");
  const sandbox = await wd.exec(() => document.querySelector("[data-testid=preview-html]").getAttribute("sandbox"));
  if (sandbox !== "") throw new Error(`html preview sandbox is ${JSON.stringify(sandbox)}`);
  await shot("media-html");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  // Outside the session's folder: refused by the host.
  const refused = await rpc("session.file.read", { id: s.id, path: "../../../etc/passwd", offset: 0, length: 16 }, local).then(() => false, () => true);
  if (!refused) throw new Error("a path outside the session folder was readable");
  // Leave the project as the next scenarios expect it (the Changes tab counts files).
  fs.rmSync(path.join(project, "out"), { recursive: true, force: true });
});

// Polish round: a session launched with an image on the first prompt (create, upload, send),
// previews of an HTML file with a relative image, highlighted code and rendered Markdown, a
// transcript with a coloured fenced block and a diff, and path chips only inside the folder.
scenario_("polish", async () => {
  const png = fs.readFileSync(path.join(project, "assets/sketch.png")).toString("base64");
  const s = await launchViaDialog({ agent: "Claude Code", cwd: project, prompt: "write the polish files", attach: png, beforeStart: () => shot("polish-new-session-attachments") });
  await waitPhase(s.id, "idle", 90_000);
  await openSession(s);
  await wd.exec(() => document.querySelectorAll(".toast button[aria-label=Dismiss]").forEach((b) => b.click()));
  await bodyHas("polish/src/app.ts");
  // The attached image reached the session: it is in the first prompt as a real image.
  await waitFor(() => wd.exec(() => [...document.querySelectorAll("[data-testid=media-image] img")].filter((i) => i.complete && i.naturalWidth > 0).length >= 1), "the attached image shows in the transcript", 30_000);
  const hist = await rpc("session.history", { id: s.id, limit: 50 }, local);
  if (!JSON.stringify(hist).includes("sketch.png")) throw new Error("the first prompt did not carry the attachment");
  await wd.exec(() => document.querySelector(".transcript").scrollTo(0, 1e9));
  // Chips: inside the folder (with the line), none for the path outside it.
  await click("[data-testid=file-ref] button", "app.ts:2");
  await waitFor(() => wd.exec(() => !!document.querySelector("[data-testid=preview-text] .hljs-keyword")), "code highlighted", 30_000);
  const focus = await wd.exec(() => document.querySelector("[data-testid=preview-text] .focus")?.getAttribute("data-ln"));
  if (focus !== "2") throw new Error(`the preview did not open at line 2 (${focus})`);
  await shot("polish-code-preview");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  const chips = await wd.exec(() => [...document.querySelectorAll("[data-testid=file-ref]")].map((c) => c.textContent));
  if (chips.some((t) => t.includes("passwd"))) throw new Error("a path outside the folder became a chip");
  await click("[data-testid=file-ref] button", "index.html");
  await waitFor(() => wd.exec(() => document.querySelector("[data-testid=preview-html]")?.getAttribute("srcdoc")?.includes("data:image/png")), "relative image inlined", 30_000);
  const doc = await wd.exec(() => document.querySelector("[data-testid=preview-html]").getAttribute("srcdoc"));
  if (doc.includes("example.com")) throw new Error("remote content reached the frame");
  await waitFor(() => wd.exec(() => /Remote content blocked: 1/.test(document.body.textContent)), "remote content note", 10_000);
  await shot("polish-html-preview");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  await click("[data-testid=file-ref] button", "guide.md");
  await el("[data-testid=preview-markdown] h1", "Guide");
  await waitFor(() => wd.exec(() => !!document.querySelector("[data-testid=preview-markdown] .hljs-built_in, [data-testid=preview-markdown] .hljs-string, [data-testid=preview-markdown] .hljs-keyword")), "fenced code in the markdown coloured", 30_000);
  await shot("polish-markdown-preview");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
  // The transcript: a coloured fenced block and the edit as a diff.
  await wd.exec(() => [...document.querySelectorAll(".tool-h")].filter((b) => b.textContent.includes("Edit")).forEach((b) => b.click()));
  await el("[data-testid=tool-diff]");
  await wd.exec(() => document.querySelector(".transcript").scrollTo(0, 1e9));
  await waitFor(() => wd.exec(() => !!document.querySelector("[data-testid=md-code] .hljs-keyword")), "transcript block coloured", 30_000);
  await shot("polish-transcript");
  const refused = await rpc("session.file.read", { id: s.id, path: "../../../etc/passwd", offset: 0, length: 16 }, local).then(() => false, () => true);
  if (!refused) throw new Error("a path outside the session folder was readable");
  fs.rmSync(path.join(project, "polish"), { recursive: true, force: true });
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
  if (!(await snapOf(s.id)).slash_commands.some((c) => c.name === "review")) return "model and mode only: this persona reports no slash commands (live-*-slash covers them)";
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
  // Its own session: the others are used by later scenarios.
  const s = await launchViaDialog({ agent: DRIVER_NAMES.codex, cwd: project, prompt: "a session to stop and delete" });
  await waitPhase(s.id, "idle");
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

scenario_("spawn", async () => {
  // A fake claude session asks to spawn a worker (spawn_agent through its Wasl MCP server); the
  // spawn gate and the first handoff are answered in the UI; the worker joins a new workstream
  // graph marked "spawned by"; the agent reports the worker's answer.
  if (REAL) return "fake claude only";
  const s = await launchViaDialog({ agent: "Claude Code", cwd: project });
  await rpc("sessions.rename", { id: s.id, display_name: "Planner" }, local);
  await waitPhase(s.id, "idle");
  await openSession(s);
  await sendPrompt("spawn a worker for 6*7");
  const gate = await el("[data-gate=spawn]", null, 60_000);
  const text = (await wd.text(gate)).replace(/\s+/g, " ");
  for (const want of ["Planner wants to start a new agent, calc", "Does the arithmetic", "what is 6*7?", "approving creates the workstream “Planner + workers”", "Marsad spawn choices"])
    if (!text.includes(want)) throw new Error(`spawn gate lacks "${want}": ${text}`);
  await shot("spawn-gate-session");
  await click("[data-gate=spawn] .choices button", "Allow once");
  // The first task goes over an "ask" connection: its handoff gate shows on the same session.
  await click("[data-gate=handoff] .choices button", "Allow once", 60_000);
  await waitFor(async () => (await snapOf(s.id))?.last_turn?.reply === "calc answered: 42", "the agent reports the worker's answer", 90_000);
  await bodyHas("calc answered: 42");
  const ws = await waitFor(async () => (await rpc("workstreams.list")).workstreams.find((w) => w.name === "Planner + workers"), "the new workstream");
  const calc = ws.nodes.find((n) => n.name === "calc");
  if (!calc?.spawned_by || !calc.session) throw new Error(`calc is not a spawned, launched node: ${JSON.stringify(calc)}`);
  await go(`#/workstreams/${ws.id}`);
  const marker = await el("[data-testid=spawned-by]", "by Planner", 30_000);
  void marker;
  await el("[data-testid=spawn-settings]", null, 20_000);
  await sleep(1200);
  await shot("spawn-graph");
  // The Spawning section: always allow, extended to handoffs (written into the document).
  await click("[data-testid=spawn-settings] [role=radio]", "Always allow");
  await waitFor(async () => (await rpc("workstreams.get", { id: ws.id })).spawn.policy === "always_allow", "spawn policy saved");
  await click("[data-testid=spawn-settings] label.chkl", "covers handoffs");
  await waitFor(async () => (await rpc("workstreams.get", { id: ws.id })).spawn.handoffs_always_allow === true, "handoff extension saved");
  await sleep(600);
  const sec = await el("[data-testid=spawn-settings]");
  await wd.exec((e) => e.scrollIntoView({ block: "start" }), sec).catch(() => {});
  await shot("spawn-inspector");
  // The list shows the spawned count.
  await go("#/workstreams");
  await bodyHas("1 spawned");
  return `${ws.name}: ${ws.nodes.length} agents`;
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
  // The "remote" login shell runs commands without profiles: the host's environment stage
  // keeps this PATH (this machine's /etc/profile would put a real CLI first).
  const loginsh = path.join(d, "bin", "loginsh");
  fs.writeFileSync(loginsh, '#!/bin/sh\nwhile [ $# -gt 1 ]; do shift; done\nexec /bin/sh -c "$1"\n', { mode: 0o755 });
  fs.writeFileSync(path.join(d, "authorized_keys"), `environment="HOME=${rh}",environment="PATH=${PATH}",environment="SHELL=${loginsh}",environment="WASIT_FAKE_SCENARIO=${scenarioPath}" ${pub}\n`);
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

// A host that signs in with a password and a second factor: sshd with keyboard-interactive
// through PAM (pam_unix, then pam_oath with a TOTP secret); both prompts are answered in the
// sign-in dialog. Changes /etc/pam.d/sshd for the run (restored in the teardown).
const otp = { port: 0, restore: null };
function totp(hex, t = Date.now()) {
  const counter = Buffer.alloc(8);
  counter.writeBigUInt64BE(BigInt(Math.floor(t / 1000 / 30)));
  const h = crypto.createHmac("sha1", Buffer.from(hex, "hex")).update(counter).digest();
  const o = h[h.length - 1] & 0xf;
  return String((h.readUInt32BE(o) & 0x7fffffff) % 1_000_000).padStart(6, "0");
}
scenario_("ssh-host-second-factor", async () => {
  if (!startSshd()) return "skipped: needs root and sshd";
  const pamOath = ["/lib/x86_64-linux-gnu/security/pam_oath.so", "/usr/lib/x86_64-linux-gnu/security/pam_oath.so"].find((p) => fs.existsSync(p));
  if (!pamOath) return "skipped: needs pam_oath (libpam-oath)";
  const d = path.join(root, "ssh");
  // Its own account: a second host entry for lab-pc's account would find lab-pc's daemon.
  otp.user = "marsadotp";
  spawnSync("useradd", ["-m", "-s", "/bin/bash", otp.user], { stdio: "ignore" });
  execFileSync("chpasswd", { input: `${otp.user}:${sshd.password}\n` });
  const secret = crypto.randomBytes(20).toString("hex");
  const users = path.join(d, "users.oath");
  fs.writeFileSync(users, `HOTP/T30/6 ${otp.user} - ${secret}\n`, { mode: 0o600 });
  otp.restore = fs.readFileSync("/etc/pam.d/sshd", "utf8");
  fs.writeFileSync("/etc/pam.d/sshd", ["auth requisite pam_unix.so", `auth required ${pamOath} usersfile=${users} window=4 digits=6`, "account required pam_unix.so", "session required pam_permit.so", ""].join("\n"));
  otp.port = sshd.port + 1;
  fs.writeFileSync(path.join(d, "sshd_otp_config"), [
    `Port ${otp.port}`, "ListenAddress 127.0.0.1", `HostKey ${path.join(d, "hostkey")}`, "UsePAM yes", "KbdInteractiveAuthentication yes",
    "PasswordAuthentication no", "PubkeyAuthentication no", "AuthenticationMethods keyboard-interactive", "StrictModes no",
    `PidFile ${path.join(d, "sshd_otp.pid")}`, "LogLevel ERROR", "",
  ].join("\n"));
  const hostpub = fs.readFileSync(path.join(d, "hostkey.pub"), "utf8").trim().split(/\s+/);
  fs.appendFileSync(sshd.knownHosts, `[127.0.0.1]:${otp.port} ${hostpub[0]} ${hostpub[1]}\n`);
  otp.child = spawn("/usr/sbin/sshd", ["-D", "-e", "-f", path.join(d, "sshd_otp_config")], { stdio: ["ignore", "ignore", fs.openSync(path.join(d, "sshd_otp.log"), "a")] });
  await sleep(500);
  await go("#/hosts");
  await button("Add host");
  await el("[role=dialog]");
  await field("Name", "secure-box");
  await field("SSH target", `${otp.user}@127.0.0.1`);
  await field("Port", String(otp.port));
  await field("Extra ssh options", `UserKnownHostsFile=${sshd.knownHosts}`);
  await click("[role=dialog] button.primary", "Add and connect");
  const answer = async (want, value) => {
    await waitFor(async () => {
      if (await wd.exec(() => !!document.querySelector(".authp-input"))) return true;
      if ((await linkOf("secure-box"))?.state?.state === "auth_needed") { await resetUi(); await go("#/hosts"); await click(".hrow[data-host]", "secure-box"); await click("button.primary", "Connect", 20_000); }
      return false;
    }, `the ${want} prompt`, 90_000);
    // What ssh asked (via Marsad's askpass), as the shell's state holds it.
    const prompt = JSON.stringify(Object.values((await state())?.hosts ?? {}).find((x) => x.entry.name === "secure-box")?.authPrompt ?? null);
    if (!new RegExp(want, "i").test(prompt)) throw new Error(`expected a ${want} prompt, got ${JSON.stringify(prompt.slice(0, 200))}`);
    if (want === "one-time") {
      // A second factor is the next step, not a retry of the password.
      if (!/"attempt":1\b/.test(prompt)) throw new Error(`the one-time code prompt counts as a retry: ${prompt}`);
      await shot("43-host-sign-in-second-factor");
    }
    await type(".authp-input", value());
    await button("Sign in");
    await waitFor(async () => !(await wd.exec(() => !!document.querySelector(".authp-input"))), "the prompt answered", 20_000).catch(() => {});
  };
  await answer("password", () => sshd.password);
  await answer("one-time", () => totp(secret));
  await waitFor(async () => (await linkOf("secure-box"))?.state?.state === "connected", "secure-box connected", 180_000);
  await resetUi();
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
    // claude reports "default" for the mode its --help calls "manual".
    const sameMode = (a, b) => a === b || (a === "default" && b === "manual") || (a === "manual" && b === "default");
    const mode = x.available_modes.find((m) => !sameMode(x.mode, m.id));
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
    const name = ["context", "cost", "status", "compact", "review", "init"].find((n) => names.includes(n)) ?? names[0];
    if (!name) throw new Error("the CLI reported no slash commands");
    const before = x.turns_completed;
    await openSession(s);
    await type("textarea[aria-label=Message]", `/${name}`);
    await el("[role=listbox][aria-label='Slash commands']");
    await wd.sendKeys(await el("textarea[aria-label=Message]"), Keys.ENTER);
    // The command runs as a turn; a long one (a free model reviewing) is interrupted after a
    // minute — it ran either way.
    await waitFor(async () => { const y = await snapOf(s.id); return y.turns_completed > before || y.phase.state === "working" || y.phase.state === "needs_you"; }, `/${name} started`, 60_000);
    const done = await settle(s.id, { answer: "reject_once", ms: 60_000 }).then(() => true, () => false);
    if (!done) { await openSession(s); await button("Interrupt"); await waitPhase(s.id, "idle", 60_000); }
    await waitFor(async () => (await snapOf(s.id)).turns_completed > before, `/${name} completed a turn`, 60_000);
    return `/${name} of ${names.length}${done ? "" : " (interrupted after a minute)"}`;
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
    await waitPhase(l.id, "idle", 300_000);
    await openSession(l);
    await bodyHas("saffron", 60_000);
    const r = await launchViaDialog({ agent: L.agent, cwd: project, prompt: ask, resumeId: a, model: L.model });
    await waitPhase(r.id, "idle", 300_000);
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

scenario_("translucent-window", async () => {
  // Settings → General → Window background. Without a compositor the shell answers that the
  // window cannot be transparent and the UI stays solid; with one (xcompmgr) the alpha surface
  // tokens apply from the first paint after a restart, and the screen shows a checkerboard
  // window through the app (captured with ImageMagick's `import`, not WebDriver).
  const have = (b) => spawnSync("sh", ["-c", `command -v ${b}`]).status === 0;
  const surface = () => wd.exec(() => document.documentElement.getAttribute("data-surface"));
  const pick = async (label) => {
    await go("#/settings?s=general");
    await click("button", label);
  };
  await rpc("store.settings.set", { key: "surface", value: "solid" });
  await pick("Translucent");
  await bodyHas("cannot show transparent windows");
  if ((await surface()) !== null) throw new Error("translucent without a compositor");
  await waitFor(async () => (await rpc("store.settings.get")).values.surface === "translucent", "the choice saved in the hub");
  if (!["xcompmgr", "display", "convert"].every(have)) {
    await pick("Solid");
    return "skipped the composited half: install xcompmgr and imagemagick";
  }
  // A checkerboard window behind the app and a compositor; the app is started again so its
  // window is created on a composited screen, above the checkerboard.
  const png = path.join(root, "backdrop.png");
  spawnSync("convert", ["-size", "400x250", "pattern:checkerboard", "-scale", "400%", "-fill", "#d9822b", "-opaque", "#999999", "-fill", "#2b7bd9", "-opaque", "#666666", png]);
  const backdrop = spawn("display", ["-geometry", "+0+0", png], { stdio: "ignore" });
  const comp = spawn("xcompmgr", [], { stdio: "ignore" });
  const restart = async () => {
    await wd.quit().catch(() => {});
    await startApp();
  };
  try {
    await sleep(800); // display and xcompmgr map/claim asynchronously; nothing to wait on.
    await restart();
    // Startup: the saved choice applies by itself (first paint from this device's cache).
    await waitFor(async () => (await surface()) === "translucent", "translucent at startup");
    if (await wd.exec(() => getComputedStyle(document.querySelector(".app")).backgroundColor).then((c) => !/(\/\s*0?\.\d+\)|,\s*0?\.\d+\))$/.test(c))) {
      throw new Error("the window layer is not translucent");
    }
    for (const theme of ["dark", "light"]) {
      await setTheme(theme);
      await waitFor(async () => (await surface()) === "translucent", `translucent in ${theme}`);
      if (launched.claude) await openSession(launched.claude);
      else await go("#/inbox");
      await sleep(600);
      if (dirs.shots) spawnSync("import", ["-window", "root", path.join(dirs.shots, `nit-translucent-${theme}.png`)]);
    }
    await setTheme("dark");
    await go("#/settings?s=general");
    await bodyHas("Translucency");
    if (dirs.shots) spawnSync("import", ["-window", "root", path.join(dirs.shots, "nit-translucent-settings.png")]);
    await pick("Solid");
    await waitFor(async () => (await surface()) === null, "solid again");
  } finally {
    comp.kill();
    backdrop.kill();
    await restart();
  }
});

// ── screenshots for the owner (--shots): every screen on believable live data ─────────────

const key = (k, mods = {}) => wd.exec((k, m) => document.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...m })), k, mods);
const launchRpc = (host, driver, prompt, extra = {}) => rpc("sessions.launch", { host, driver, cwd: extra.cwd ?? null, model: null, mode: null, first_prompt: prompt, resume: { kind: "new" }, labels: {}, workstream: null, overrides: { items: [], env: [], disallowed_tools: [] } });
const sessionPage = async (host, id, tab = "") => { await go(`#/session/${encodeURIComponent(host)}/${id}${tab ? `?tab=${tab}` : ""}`); await el(".sd-head"); };

// Live media with real claude (Haiku): it sees an attached PNG, then writes an SVG that
// opens in the preview.
async function dropFile(file, mime) {
  const b64 = fs.readFileSync(file).toString("base64");
  await wd.exec((b64, name, mime) => {
    const dt = new DataTransfer();
    dt.items.add(new File([Uint8Array.from(atob(b64), (c) => c.charCodeAt(0))], name, { type: mime }));
    const t = document.querySelector(".composer");
    t.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
    t.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, b64, path.basename(file), mime);
}

scenario_("live-claude-media", async () => {
  const s = launched["live-claude"];
  if (!s) return "no session";
  await openSession(s);
  await dropFile(path.join(project, "assets/sketch.png"), "image/png");
  await el("[data-testid=pending-attachment]", "sketch.png");
  // The upload runs before the prompt goes out: wait for this turn, not the previous one.
  const turnOver = async (before) => {
    await waitFor(async () => { const x = await snapOf(s.id); return x && (x.turns_completed > before || x.phase.state !== "idle"); }, "the turn started", 120_000);
    await settle(s.id, { answer: "allow_once" });
    await waitFor(async () => (await snapOf(s.id))?.turns_completed > before, "the turn finished", 180_000);
  };
  let n = (await snapOf(s.id)).turns_completed;
  await sendPrompt("What kind of picture is attached? Answer in at most six words.");
  await turnOver(n);
  const said = await wd.exec(() => [...document.querySelectorAll(".msg-ai")].map((m) => m.innerText).join(" ").toLowerCase());
  if (!/bar|chart|graph/.test(said)) throw new Error(`claude did not describe the chart: ${said.slice(-300)}`);
  n = (await snapOf(s.id)).turns_completed;
  await sendPrompt("Use the Write tool to create live_chart.svg in the current directory: a small SVG bar chart with three bars. Then reply with the file name.");
  await turnOver(n);
  if (!fs.existsSync(path.join(project, "live_chart.svg"))) throw new Error("live_chart.svg was not written");
  await wd.exec(() => document.querySelectorAll(".toast button[aria-label=Dismiss]").forEach((b) => b.click()));
  await click("[data-testid=file-ref] button", "live_chart.svg", 30_000);
  await waitFor(() => wd.exec(() => document.querySelector("[data-testid=preview]")?.getAttribute("data-kind") === "svg" && document.querySelector("[data-testid=preview-image]")?.naturalWidth > 0), "live svg preview", 20_000);
  await shot("media-live-claude-svg");
  await wd.exec(() => document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
}, { real: true });

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
  if (lc) { await openSession(lc); await sendPrompt(shell("claude", "rm -rf build/cache")); await waitPhase(lc.id, "needs_you", 120_000).catch(() => {}); }
  const lo = launched["live-opencode"];
  if (lo) { await openSession(lo); await sendPrompt("Use the ask_user tool (marsad MCP server) to ask me which export format to ship first, as a single choice between PDF, CSV and XLSX, each with a one-line description. Then reply with my choice."); await waitFor(async () => (await pendingGate(lo.id))?.gate_type === "question", "ask_user question", 120_000).catch(() => {}); }
  await sleep(1500);

  await setTheme("dark");
  await go("#/inbox");
  await sleep(1500);
  await shot("dark-01-inbox");
  await shot("fix-inbox-question");
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
  await go(`#/workstreams/${ws.id}`); await el("[data-gate=handoff]", null, 20_000).catch(() => {}); await sleep(1200); await shot("dark-12-workstream-graph-running-handoff-pending"); await shot("fix-graph-labels");
  await go(`#/workstreams/${draft.id}`); await sleep(1200); await shot("dark-13-workstream-design-mode");
  // Hosts, config, settings.
  await go("#/hosts"); await sleep(800); await click(".hrow[data-host]", "build-box").catch(() => {}); await sleep(600); await shot("dark-14-hosts");
  // Overrides on top of claude's own config: a skill turned off, an MCP server added.
  const cbase = await rpc("agentconfig.base", { driver: "claude", project: null, refresh: false }, local).catch(() => null);
  const skill = cbase?.items.find((i) => i.kind === "skill")?.name;
  await rpc("store.layers.set", { scope: { scope: "global" }, layer: { items: [
    ...(skill ? [{ kind: "skill", name: skill, driver: "claude", action: "disable" }] : []),
    { kind: "mcp_server", name: "docs-search", driver: null, action: "set", value: { kind: "mcp_server", server: { transport: "http", url: "https://docs.example.com/mcp", headers: {} } } },
  ], env: [], disallowed_tools: [] } });
  await go("#/config"); await sleep(800);
  await click("button", "Claude Code").catch(() => {});
  await click("button[aria-label^='Effective for'], [aria-label^='Host']", null, 5000).then(() => click("[role=option]", "vm", 5000)).catch(() => {});
  await sleep(1500); await shot("dark-15-config-agent-configuration");
  // The Global layer selected: its added MCP server offers "remove", the skill it turned off "reset".
  await click("button[title='Edit the Global layer']", null, 5000).catch(() => {});
  await sleep(800); await shot("fix-config-chips");
  await go("#/settings"); await button("Notifications"); await sleep(600); await shot("dark-16-settings-notifications");
  // Palette and shortcut sheet.
  await go("#/inbox"); await sleep(500);
  await key("k", { ctrlKey: true }); await sleep(700); await shot("dark-17-command-palette"); await key("Escape"); await resetUi();
  await key("?"); await sleep(700); await shot("dark-18-shortcut-sheet"); await key("Escape"); await resetUi();
  // Light.
  await setTheme("light");
  await go("#/inbox"); await sleep(1200); await shot("light-01-inbox");
  await go(`#/workstreams/${ws.id}`); await sleep(1200); await shot("light-02-workstream-graph-running");
  await setTheme("dark");
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
  if (dirs.shots) await setTheme("dark");
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
  otp.child?.kill();
  if (otp.restore != null) fs.writeFileSync("/etc/pam.d/sshd", otp.restore);
  if (otp.user) {
    spawnSync("pkill", ["-f", `/home/${otp.user}/`], { stdio: "ignore" });
    spawnSync("userdel", ["-r", otp.user], { stdio: "ignore" });
  }
  if (sshd.pwUser) {
    // Its remote daemon runs as that user (under its home): stop it, or userdel refuses.
    spawnSync("pkill", ["-f", `/home/${sshd.pwUser}/`], { stdio: "ignore" });
    spawnSync("userdel", ["-r", sshd.pwUser], { stdio: "ignore" });
  }
  spawnSync(bin.agent, ["stop", "--data", dirs.data], { stdio: "ignore" });
  // Sessions outlive the daemon by design (their shims hold them); end this run's ones.
  spawnSync("pkill", ["-f", `${dirs.data}/machines`], { stdio: "ignore" });
  // The ssh hosts' daemons and sessions live under this run's temp dir too.
  spawnSync("pkill", ["-f", root], { stdio: "ignore" });
  code = failed ? 1 : 0;
  console.log(`\n${results.filter((r) => r.ok).length} passed, ${failed} failed${KEEP || failed ? ` — data in ${root}` : ""}`);
  if (!KEEP && !failed) fs.rmSync(root, { recursive: true, force: true });
}
void state;
void sessionByTitle;
process.exit(code);
