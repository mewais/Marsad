/**
 * Typed wrappers over @tauri-apps/api invoke() for every bridge command.
 *
 * Browser/test mode: when the Tauri runtime is absent, falls back to the JS
 * mock fixture so Vitest + Playwright work without the Rust side.
 */

import type { AppSnapshot, SessionSummary } from "./types";
import { MOCK_SNAPSHOT } from "./mockFixture";

// ---------------------------------------------------------------------------
// Tauri detection
// ---------------------------------------------------------------------------

function isTauri(): boolean {
  // Tauri v2 always injects `__TAURI_INTERNALS__` into the webview (that's what
  // `invoke()` uses). `__TAURI__` only exists when `app.withGlobalTauri` is
  // enabled, which it is NOT here — so checking only `__TAURI__` made the real
  // app fall back to mock data. Detect either, preferring the internals global.
  return (
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

// ---------------------------------------------------------------------------
// Lazy Tauri invoke loader (avoids importing @tauri-apps/api at module level
// so tests that don't have Tauri shims don't blow up)
// ---------------------------------------------------------------------------

type InvokeFn = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

async function getInvoke(): Promise<InvokeFn> {
  if (!isTauri()) {
    throw new Error("Not in Tauri");
  }
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke as InvokeFn;
}

// ---------------------------------------------------------------------------
// Mock state (for browser/test mode — mirrors Rust MockState mutations)
// ---------------------------------------------------------------------------

let _mockSnapshot: AppSnapshot = { ...MOCK_SNAPSHOT };

export function _resetMock() {
  _mockSnapshot = { ...MOCK_SNAPSHOT };
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export async function snapshot(): Promise<AppSnapshot> {
  if (!isTauri()) return _mockSnapshot;
  const invoke = await getInvoke();
  return invoke("snapshot") as Promise<AppSnapshot>;
}

export async function listSessions(project: string): Promise<SessionSummary[]> {
  if (!isTauri()) {
    return [
      { id: "sess-aaaa-1111", snippet: "Add a login endpoint to the API", modified: 1719000000000 },
      { id: "sess-bbbb-2222", snippet: "Refactor the auth middleware", modified: 1718990000000 },
    ];
  }
  const invoke = await getInvoke();
  return invoke("list_sessions", { project }) as Promise<SessionSummary[]>;
}

export async function launchSession(args: {
  project: string;
  host_id: string;
  model: string;
  posture: "ask" | "auto" | "full";
  first_prompt?: string;
  resume_session_id?: string;
  continue_recent?: boolean;
}): Promise<string> {
  if (!isTauri()) return "mock-session-new";
  const invoke = await getInvoke();
  return invoke("launch_session", args) as Promise<string>;
}

export async function resumeSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("resume_session", { id });
}

export async function attachSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("attach_session", { id });
}

export async function stopSession(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("stop_session", { id });
}

export async function sendPrompt(id: string, text: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("send_prompt", { id, text });
}

export async function interrupt(id: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("interrupt", { id });
}

export async function answerGate(args: {
  session_id: string;
  decision_id: string;
  allow: boolean;
  deny_reason?: string;
}): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("answer_gate", args);
}

export async function runSlash(id: string, cmd: string): Promise<void> {
  if (!isTauri()) return;
  const invoke = await getInvoke();
  await invoke("run_slash", { id, cmd });
}

export async function setWorkspace(id: string): Promise<void> {
  if (!isTauri()) {
    _mockSnapshot = { ..._mockSnapshot, active_ws: id };
    return;
  }
  const invoke = await getInvoke();
  await invoke("set_workspace", { id });
}

// ---------------------------------------------------------------------------
// Event listener for vm-update (Tauri) / no-op in browser
// ---------------------------------------------------------------------------

export type UnlistenFn = () => void;

export async function listenVmUpdate(
  handler: (event: unknown) => void
): Promise<UnlistenFn> {
  if (!isTauri()) {
    // In browser/test mode there are no live events; return a no-op unlisten.
    return () => {};
  }
  const { listen } = await import("@tauri-apps/api/event");
  return listen("vm-update", (event) => handler(event.payload));
}

// ---------------------------------------------------------------------------
// Slash catalog (static — mirrors Rust all_slash_entries())
// ---------------------------------------------------------------------------

export const SLASH_CATALOG = [
  { name: "/clear",    desc: "Clear conversation history" },
  { name: "/compact",  desc: "Compact context to save tokens" },
  { name: "/cost",     desc: "Show token usage and cost" },
  { name: "/model",    desc: "Switch the active model" },
  { name: "/config",   desc: "Open configuration" },
  { name: "/init",     desc: "Initialize CLAUDE.md for this project" },
  { name: "/review",   desc: "Review recent changes" },
  { name: "/agents",   desc: "List running sub-agents" },
  { name: "/resume",   desc: "Resume a previous conversation" },
  { name: "/terminal", desc: "Open integrated terminal" },
  { name: "/help",     desc: "Show available commands" },
] as const;

export function filterSlash(query: string) {
  if (!query) return [...SLASH_CATALOG];
  return SLASH_CATALOG.filter((e) => e.name.startsWith(query));
}
