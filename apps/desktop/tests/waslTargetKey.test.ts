/**
 * waslTargetKey.test.ts — W2b: resume context threading for Wasl driver launches.
 *
 * Tests the `buildWaslTargetKey` pure helper that both NodeModal.handleLaunch
 * and GraphScreenConnected.onConfirmDraft use to construct the `wasl_target_key`.
 *
 * Contract verified:
 *   - resume: false → context has no `resume` key (fresh session)
 *   - resume: true, selectedResume="continue" → most-recent session id in context
 *   - resume: true, selectedResume=<id> → that id in context
 *   - `protocol` matches the waslProtocol() of the driver
 *   - Other context fields (cwd, model, session_mode, peers) are always present
 *   - "continue" with empty sessions list → no resume (no sessions to resume)
 *   - `key` = nodeId in the output
 */
import { describe, it, expect } from "vitest";
import { buildWaslTargetKey } from "../src/bridge/waslTargetKey";
import type { DiscoveredSession } from "../src/bridge/waslTargetKey";

// ---------------------------------------------------------------------------
// Test data
// ---------------------------------------------------------------------------

const session1: DiscoveredSession = {
  id: "sess-uuid-most-recent",
  snippet: "Fix the login endpoint",
  modified_ms: 1720000000000,
  project: "/work/myproj",
};

const session2: DiscoveredSession = {
  id: "sess-uuid-older",
  snippet: "Add authentication middleware",
  modified_ms: 1719000000000,
  project: "/work/myproj",
};

const sessions = [session1, session2]; // ordered newest-first

const basePeers = [{ agent_id: "node-peer-1", host_id: "local" }];

// ---------------------------------------------------------------------------
// Fresh launch (resume: false) — no resume key
// ---------------------------------------------------------------------------

describe("buildWaslTargetKey — fresh launch (resume: false)", () => {
  it("omits context.resume when resume is false", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-opus-4-5",
      "plan",
      [],
      false,
      "continue",
      sessions
    );
    expect(key.context.resume).toBeUndefined();
  });

  it("sets cwd, model, session_mode, peers correctly for fresh launch", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-opus-4-5",
      "plan",
      basePeers,
      false,
      "continue",
      sessions
    );
    expect(key.context.cwd).toBe("/work/proj");
    expect(key.context.model).toBe("claude-opus-4-5");
    expect(key.context.session_mode).toBe("plan");
    expect(key.context.peers).toEqual(basePeers);
  });

  it("key = nodeId for fresh launch", () => {
    const key = buildWaslTargetKey(
      "node-xyz",
      "codex",
      "/home/user/proj",
      "codex-r1",
      "",
      [],
      false,
      "continue",
      []
    );
    expect(key.key).toBe("node-xyz");
  });

  it("protocol passes through correctly for fresh launch", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "opencode",
      "/w/p",
      "",
      "",
      [],
      false,
      "continue",
      []
    );
    expect(key.protocol).toBe("opencode");
  });
});

// ---------------------------------------------------------------------------
// Continue most-recent (resume: true, selectedResume="continue")
// ---------------------------------------------------------------------------

describe('buildWaslTargetKey — continue most-recent (selectedResume="continue")', () => {
  it("sets context.resume to the first (most recent) session id", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-opus-4-5",
      "",
      [],
      true,
      "continue",
      sessions
    );
    expect(key.context.resume).toBe("sess-uuid-most-recent");
  });

  it("still populates cwd and model alongside resume", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-sonnet",
      "plan",
      [],
      true,
      "continue",
      sessions
    );
    expect(key.context.cwd).toBe("/work/proj");
    expect(key.context.model).toBe("claude-sonnet");
    expect(key.context.session_mode).toBe("plan");
  });

  it("no resume when sessions list is empty (nothing to resume)", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-opus-4-5",
      "",
      [],
      true,
      "continue",
      [] // empty — no most-recent
    );
    // With no sessions, there is nothing to resume — omit the key.
    expect(key.context.resume).toBeUndefined();
  });

  it("uses sessions[0].id (not sessions[1].id) — always the most recent", () => {
    // session2 is "older" — sessions[0] must always be taken (newest-first order).
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/w",
      "",
      "",
      [],
      true,
      "continue",
      [session1, session2]
    );
    expect(key.context.resume).toBe(session1.id);
    expect(key.context.resume).not.toBe(session2.id);
  });
});

// ---------------------------------------------------------------------------
// Specific session chosen (resume: true, selectedResume = <session-id>)
// ---------------------------------------------------------------------------

describe("buildWaslTargetKey — specific session chosen", () => {
  it("sets context.resume to the explicitly chosen session id", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/work/proj",
      "claude-opus-4-5",
      "",
      [],
      true,
      "sess-uuid-older", // specific pick
      sessions
    );
    expect(key.context.resume).toBe("sess-uuid-older");
  });

  it("picks the session id even when it is not sessions[0]", () => {
    // The user explicitly picked the older session, not the most recent.
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/w",
      "",
      "",
      [],
      true,
      session2.id,
      [session1, session2]
    );
    expect(key.context.resume).toBe(session2.id);
  });
});

// ---------------------------------------------------------------------------
// codex protocol — wasl-codex driver maps to "codex"
// ---------------------------------------------------------------------------

describe("buildWaslTargetKey — wasl-codex (protocol='codex')", () => {
  it("protocol='codex' and resume thread correctly for codex", () => {
    const key = buildWaslTargetKey(
      "node-codex-1",
      "codex",
      "/work/codex-proj",
      "codex-r1",
      "",
      [],
      true,
      "thread-codex-abc",
      []
    );
    expect(key.protocol).toBe("codex");
    expect(key.context.resume).toBe("thread-codex-abc");
    expect(key.key).toBe("node-codex-1");
  });

  it("fresh codex launch has no resume", () => {
    const key = buildWaslTargetKey(
      "node-codex-2",
      "codex",
      "/work/codex-proj",
      "codex-r1",
      "",
      [],
      false,
      "continue",
      []
    );
    expect(key.protocol).toBe("codex");
    expect(key.context.resume).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// opencode protocol regression
// ---------------------------------------------------------------------------

describe("buildWaslTargetKey — wasl-opencode (protocol='opencode')", () => {
  it("protocol='opencode' and resume set for opencode driver", () => {
    const opencodeSess: DiscoveredSession = {
      id: "ses_opencode_abc",
      snippet: "Fix the build",
      modified_ms: 1720000001000,
      project: "/w/oc-proj",
    };
    const key = buildWaslTargetKey(
      "node-oc-1",
      "opencode",
      "/w/oc-proj",
      "claude-sonnet",
      "",
      [],
      true,
      "continue",
      [opencodeSess]
    );
    expect(key.protocol).toBe("opencode");
    expect(key.context.resume).toBe("ses_opencode_abc");
  });
});

// ---------------------------------------------------------------------------
// peers pass-through (W2a + W2b combined scenario)
// ---------------------------------------------------------------------------

describe("buildWaslTargetKey — peers + resume combined", () => {
  it("peers list is preserved alongside resume", () => {
    const key = buildWaslTargetKey(
      "node-abc",
      "claude",
      "/w",
      "",
      "",
      [{ agent_id: "peer-1", host_id: "host-a" }, { agent_id: "peer-2", host_id: "host-b" }],
      true,
      "sess-uuid-most-recent",
      sessions
    );
    expect(key.context.peers).toEqual([
      { agent_id: "peer-1", host_id: "host-a" },
      { agent_id: "peer-2", host_id: "host-b" },
    ]);
    expect(key.context.resume).toBe("sess-uuid-most-recent");
  });
});
