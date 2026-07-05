/**
 * Mock BoardView fixture — used by tests + dev (no Tauri runtime required).
 * See docs/specs/ui-board.md §Mock fixture for the coverage requirements this
 * fixture must satisfy.
 */
import type { BoardView } from "./workstreamTypes";

export const MOCK_BOARD: BoardView = {
  summary: { workstreams: 3, sessions: 6, hosts: 3 },

  host_chips: [
    { id: "localhost", label: "localhost", kind: "local", status: "connected" },
    { id: "wsl-box", label: "wsl-dev", kind: "wsl", status: "reconnecting" },
    { id: "build-host", label: "build-host", kind: "remote", status: "connected" },
  ],

  link_chips: [
    {
      a: "localhost",
      b: "build-host",
      status: "degraded",
      note: "packet loss · 38%",
    },
  ],

  attention: [
    {
      id: "ws-auth-refactor",
      name: "Refactor auth middleware",
      goal: "Migrate session auth to the new middleware stack",
      head_state: "needs",
      driver_dots: ["claude"],
      host_chips: ["localhost"],
      session_count: 1,
      counts: { working: 0, needs: 1, error: 0, done: 0 },
      needs_attention: true,
      is_single_node: true,
      headline: "Refactor auth middleware wants to run: npx prisma migrate deploy",
      cta_label: "Review",
    },
    {
      id: "ws-build-pipeline",
      name: "Build pipeline handoff",
      goal: "Cross-host handoff waiting — build artifact → test host",
      head_state: "error",
      driver_dots: ["codex", "claude"],
      host_chips: ["localhost", "build-host"],
      session_count: 2,
      counts: { working: 0, needs: 0, error: 1, done: 1 },
      needs_attention: true,
      is_single_node: false,
      headline: "Cross-host handoff waiting — build artifact → test host.",
      cta_label: "Review handoff",
    },
  ],

  all: [
    {
      id: "ws-auth-refactor",
      name: "Refactor auth middleware",
      goal: "Migrate session auth to the new middleware stack",
      head_state: "needs",
      driver_dots: ["claude"],
      host_chips: ["localhost"],
      session_count: 1,
      counts: { working: 0, needs: 1, error: 0, done: 0 },
      needs_attention: true,
      is_single_node: true,
    },
    {
      id: "ws-build-pipeline",
      name: "Build pipeline handoff",
      goal: "Cross-host handoff waiting — build artifact → test host",
      head_state: "error",
      driver_dots: ["codex", "claude"],
      host_chips: ["localhost", "build-host"],
      session_count: 2,
      counts: { working: 0, needs: 0, error: 1, done: 1 },
      needs_attention: true,
      is_single_node: false,
    },
    {
      id: "ws-docs-refresh",
      name: "Docs refresh",
      goal: "Bring the onboarding docs up to date with the new CLI flags",
      head_state: "working",
      driver_dots: ["claude", "codex", "gemini"],
      host_chips: ["localhost", "wsl-box", "build-host"],
      session_count: 3,
      counts: { working: 2, needs: 0, error: 0, done: 1 },
      needs_attention: false,
      is_single_node: false,
    },
  ],
};
